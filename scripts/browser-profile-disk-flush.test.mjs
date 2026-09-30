import assert from "node:assert/strict";
import test from "node:test";
import {
  BROWSER_PROFILE_DISK_MOUNT,
  browserProfileDiskDurability,
  flushAndUnmountBrowserProfileDisk,
  guestProcessIdsHoldingProfileDisk,
  quitGuestProfileDiskWriters,
} from "./browser-selkies-control-service.mjs";

test("flush treats a missing profile mount as already unmounted", () => {
  const commands = [];
  const result = flushAndUnmountBrowserProfileDisk({
    exec: (command, args = []) => {
      commands.push([command, ...args]);
      return { status: 0 };
    },
    readMounts: () => "ext4 / rw 0 0\n",
  });
  assert.deepEqual(commands, [["sync"]]);
  assert.deepEqual(result, {
    flushed: true,
    unmounted: true,
    already_absent: true,
  });
});

test("flush syncs then unmounts the guest profile disk", () => {
  const commands = [];
  const result = flushAndUnmountBrowserProfileDisk({
    exec: (command, args = []) => {
      commands.push([command, ...args]);
      return { status: 0 };
    },
    readMounts: () => `/dev/vdb ${BROWSER_PROFILE_DISK_MOUNT} ext4 rw,noatime 0 0\n`,
  });
  assert.deepEqual(commands, [["sync"], ["umount", BROWSER_PROFILE_DISK_MOUNT]]);
  assert.deepEqual(result, {
    flushed: true,
    unmounted: true,
    already_absent: false,
  });
});

test("flush propagates a readMounts exception", () => {
  assert.throws(
    () =>
      flushAndUnmountBrowserProfileDisk({
        exec: () => ({ status: 0 }),
        readMounts: () => {
          throw new Error("proc mounts are unreadable");
        },
      }),
    /proc mounts are unreadable/,
  );
});

test("flush fails closed when umount fails", () => {
  assert.throws(
    () =>
      flushAndUnmountBrowserProfileDisk({
        exec: (command) => ({ status: command === "umount" ? 1 : 0 }),
        readMounts: () => `/dev/vdb ${BROWSER_PROFILE_DISK_MOUNT} ext4 rw 0 0\n`,
        umountAttempts: 1,
      }),
    /Browser profile disk umount failed/,
  );
});

test("flush retries a busy umount until the profile disk releases", () => {
  let umounts = 0;
  const sleeps = [];
  const result = flushAndUnmountBrowserProfileDisk({
    exec: (command) => {
      if (command === "umount") {
        umounts += 1;
        return { status: umounts < 3 ? 1 : 0 };
      }
      return { status: 0 };
    },
    readMounts: () => `/dev/vdb ${BROWSER_PROFILE_DISK_MOUNT} ext4 rw 0 0\n`,
    sleep: (ms) => sleeps.push(ms),
    umountAttempts: 4,
    umountRetryMs: 250,
  });
  assert.equal(umounts, 3);
  assert.deepEqual(sleeps, [250, 250]);
  assert.deepEqual(result, {
    flushed: true,
    unmounted: true,
    already_absent: false,
  });
});

test("guest writer discovery skips the control service pid", () => {
  const ids = guestProcessIdsHoldingProfileDisk({
    listProc: () => ["1", "22", "33"],
    readCmdline: (pid) => {
      if (pid === "1") return "init\0";
      if (pid === "22") return "/opt/elastos/bin/browser-native-proxy-engine\0";
      return `--user-data-dir=${BROWSER_PROFILE_DISK_MOUNT}/profiles/key\0`;
    },
    selfPid: 33,
  });
  assert.deepEqual(ids, [22]);
});

test("guest writer quit waits for Browser.close exit before TERM", async () => {
  const commands = [];
  const leftover = await quitGuestProfileDiskWriters({
    exec: (command, args = []) => {
      commands.push([command, ...args]);
      return { status: 0 };
    },
    listProc: () => ["9"],
    readCmdline: () => "browser-native-proxy-engine\0",
    pidAlive: () => false,
    nowMs: (() => {
      let now = 0;
      return () => {
        now += 10;
        return now;
      };
    })(),
    sleep: () => {},
    graceMs: 4000,
    timeoutMs: 1,
  });
  assert.deepEqual(leftover, []);
  assert.deepEqual(commands, []);
});

test("guest writer quit keeps the persist grace after writers already exited", async () => {
  const sleeps = [];
  let now = 0;
  const leftover = await quitGuestProfileDiskWriters({
    exec: () => ({ status: 0 }),
    listProc: () => ["9"],
    readCmdline: () => "browser-native-proxy-engine\0",
    pidAlive: () => false,
    nowMs: () => now,
    sleep: (ms) => {
      sleeps.push(ms);
      now += ms;
    },
    graceMs: 4000,
    timeoutMs: 1,
  });
  assert.deepEqual(leftover, []);
  assert.equal(sleeps.reduce((sum, value) => sum + value, 0), 4000);
  assert.ok(now >= 4000);
});

test("guest writer quit sends TERM then KILL for leftover pids", async () => {
  const commands = [];
  const leftover = await quitGuestProfileDiskWriters({
    exec: (command, args = []) => {
      commands.push([command, ...args]);
      return { status: 0 };
    },
    listProc: () => ["9"],
    readCmdline: () => "browser-native-proxy-engine\0",
    pidAlive: () => true,
    nowMs: (() => {
      let now = 0;
      return () => {
        now += 5000;
        return now;
      };
    })(),
    sleep: () => {},
    graceMs: 0,
    timeoutMs: 1,
  });
  assert.deepEqual(leftover, [9]);
  assert.deepEqual(commands, [
    ["kill", "-TERM", "9"],
    ["kill", "-KILL", "9"],
  ]);
  assert.deepEqual(
    browserProfileDiskDurability(
      { flushed: true, unmounted: true, already_absent: false },
      leftover,
    ),
    {
      ok: false,
      profile_disk_flushed: false,
      profile_disk_unmounted: true,
      leftover_writers_killed: true,
      error:
        "Guest profile disk writers required SIGKILL; application writes are unproven",
    },
  );
});
