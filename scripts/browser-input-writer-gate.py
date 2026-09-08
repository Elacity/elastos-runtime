"""Serialize Selkies input against a bounded Runtime operator writer lease.

The socket stays inside the guest. Its acknowledgement follows all prior native
input calls. Media retains its own callbacks and connections.
"""
import json
import os
import re
import socket
import stat
import threading
import time


class InputWriterGate:
    def __init__(self, native_input, path, clock=time.monotonic):
        self.native_input = native_input
        self.handler, self.path, self.clock = native_input.on_message, path, clock
        self.lock = threading.Lock()
        self.lease = None
        self.pending_path = path + ".pending"
        # /run belongs to this Engine VM. A producer restart cannot erase an
        # unresolved CDP effect; exact late ACK settlement or VM teardown owns it.
        try:
            fd = os.open(self.pending_path, os.O_RDONLY | os.O_NOFOLLOW)
        except FileNotFoundError:
            pass
        else:
            with os.fdopen(fd, "rb") as pending:
                metadata = os.fstat(pending.fileno())
                if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077 or metadata.st_size > 1024:
                    raise ValueError("invalid pending writer ownership")
                record = json.loads(pending.read(1025))
            if (set(record) != {"admission_id", "effect_id"}
                    or not isinstance(record["admission_id"], str) or not re.fullmatch(r"[a-f0-9-]{32,36}", record["admission_id"])
                    or not isinstance(record["effect_id"], str) or not re.fullmatch(r"[a-f0-9]{32}", record["effect_id"])):
                raise ValueError("invalid pending writer receipt")
            self.lease = {"id": record["admission_id"], "expires": clock(),
                          "pending_effect": record["effect_id"], "release_requested": True}
        self.retired = set()
        self.keys = set()
        self.closed = False
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        # The producer owns this pathname for its entire process lifetime.
        # A pre-existing live endpoint is an error, not a replacement target.
        self.server.bind(path)
        os.chmod(path, 0o600)
        self.server.listen(4)
        self.server.settimeout(0.2)
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _active(self):
        if self.lease and (self.clock() >= self.lease["expires"] or self.lease["release_requested"]):
            if len(self.retired) < 128:
                self.retired.add(self.lease["id"])
            # Expiry retires authority, but an unacknowledged native effect
            # keeps human input blocked until exact settlement or Engine teardown.
            if self.lease["pending_effect"] is None:
                self.lease = None
        return self.lease

    def on_message(self, message):
        with self.lock:
            if self.closed:
                return
            if not isinstance(message, str) or len(message) > 131072:
                return
            command = message.split(",", 1)[0]
            fields = message.split(",")
            key = None
            if command in ("kd", "ku"):
                if len(fields) != 2 or len(fields[1]) > 16:
                    return
                try:
                    key = int(fields[1])
                except ValueError:
                    return
                if not 0 <= key <= 0xffffffff or (command == "kd" and key not in self.keys and len(self.keys) >= 256):
                    return
            if self._active() and command != "pong":
                return
            self.handler(message)
            if command == "kd":
                self.keys.add(key)
            elif command == "ku":
                self.keys.discard(key)
            elif command == "kr":
                self.keys.clear()

    def command(self, request):
        if not isinstance(request, dict) or set(request) - {"command", "admission_id", "duration_ms", "effect_id"}:
            raise ValueError("invalid writer command")
        lease_id = request.get("admission_id", "")
        if not isinstance(lease_id, str) or not re.fullmatch(r"[a-f0-9-]{32,36}", lease_id):
            raise ValueError("invalid writer identity")
        if not self.lock.acquire(timeout=1):
            raise ValueError("writer busy")
        try:
            if self.closed:
                raise ValueError("writer closed")
            active = self._active()
            command = request.get("command")
            if command == "release":
                if lease_id not in self.retired and len(self.retired) < 128:
                    self.retired.add(lease_id)
                if active and active["id"] == lease_id:
                    active["release_requested"] = True
            elif command in ("begin", "settle"):
                effect_id = request.get("effect_id", "")
                if not isinstance(effect_id, str) or not re.fullmatch(r"[a-f0-9]{32}", effect_id):
                    raise ValueError("invalid effect identity")
                if command == "begin":
                    if (not active or active["id"] != lease_id or active["release_requested"]
                            or self.clock() >= active["expires"] or active["pending_effect"] not in (None, effect_id)):
                        raise ValueError("writer effect unavailable")
                    if active["pending_effect"] is None:
                        fd = os.open(self.pending_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                        with os.fdopen(fd, "w") as pending:
                            json.dump({"admission_id": lease_id, "effect_id": effect_id}, pending)
                            pending.flush()
                            os.fsync(pending.fileno())
                    active["pending_effect"] = effect_id
                elif active:
                    if active["id"] != lease_id or active["pending_effect"] not in (None, effect_id):
                        raise ValueError("writer effect mismatch")
                    if active["pending_effect"] is not None:
                        os.unlink(self.pending_path)
                    active["pending_effect"] = None
            elif command == "acquire":
                duration = request.get("duration_ms")
                if type(duration) is not int or not 2000 <= duration <= 30000:
                    raise ValueError("invalid writer duration")
                if lease_id in self.retired or (active and active["id"] != lease_id) or len(self.retired) >= 128:
                    raise ValueError("writer unavailable")
                if not active:
                    # This acknowledgement uses the X server input barrier.
                    # A uinput-backed deployment needs its own kernel barrier.
                    if getattr(self.native_input, "uinput_mouse_socket_path", None):
                        raise ValueError("writer input backend unavailable")
                    deadline = self.clock() + duration / 1000
                    # Settle native held inputs before acknowledging the handoff.
                    for key in sorted(self.keys):
                        self.native_input.send_x11_keypress(key, down=False)
                    self.keys.clear()
                    # Call the native operation directly: on_message catches
                    # pointer failures and therefore cannot acknowledge cleanup.
                    self.native_input.send_x11_mouse(0, 0, 0, 0, relative=True)
                    self.native_input.xdisplay.sync()
                    if self.clock() >= deadline:
                        self.retired.add(lease_id)
                        raise ValueError("writer deadline")
                    self.lease = {"id": lease_id, "expires": deadline, "pending_effect": None, "release_requested": False}
            elif command != "check":
                raise ValueError("invalid writer command")
            active = self._active()
            return {"schema": "elastos.browser.input-writer/v1", "admission_id": lease_id,
                    "active": bool(active and active["id"] == lease_id and not active["release_requested"] and self.clock() < active["expires"]),
                    "held": bool(active and active["id"] == lease_id),
                    "pending_effect": active["pending_effect"] if active and active["id"] == lease_id else None}
        finally:
            self.lock.release()

    def _serve(self):
        while not self.closed:
            try:
                connection, _ = self.server.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with connection:
                connection.settimeout(1)
                try:
                    raw = bytearray()
                    while b"\n" not in raw and len(raw) <= 1024:
                        data = connection.recv(1025 - len(raw))
                        if not data:
                            break
                        raw.extend(data)
                    if not raw.endswith(b"\n") or len(raw) > 1024 or raw.count(b"\n") != 1:
                        raise ValueError("invalid writer framing")
                    result = self.command(json.loads(raw))
                except Exception:
                    result = {"error": "input_writer_unavailable"}
                try:
                    connection.sendall(json.dumps(result, separators=(",", ":")).encode() + b"\n")
                except OSError:
                    pass

    def close(self):
        with self.lock:
            self.closed = True
            self.lease = None
        self.server.close()
        self.thread.join(timeout=2)
        try:
            os.unlink(self.path)
        except FileNotFoundError:
            pass
