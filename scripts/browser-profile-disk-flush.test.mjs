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

test("flush stops before unmount when sync fails", () => {
  const commands = [];
  assert.throws(
    () =>
      flushAndUnmountBrowserProfileDisk({
        exec: (command, args = []) => {
          commands.push([command, ...args]);
          return { status: 1 };
        },
        readMounts: () => `/dev/vdb ${BROWSER_PROFILE_DISK_MOUNT} ext4 rw 0 0\n`,
      }),
    /Browser profile disk sync failed/,
  );
  assert.deepEqual(commands, [["sync"]]);
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

for (const { name, processIds, exitsAt, elapsed } of [
  { name: "skips grace when discovery is empty", processIds: [], exitsAt: 0, elapsed: 0 },
  { name: "skips grace when writers already exited", processIds: ["9"], exitsAt: 0, elapsed: 0 },
  { name: "ends grace when the writer exits", processIds: ["9"], exitsAt: 150, elapsed: 150 },
]) {
  test(`guest writer quit ${name}`, async () => {
    const commands = [];
    let now = 0;
    const leftover = await quitGuestProfileDiskWriters({
      exec: (command, args = []) => {
        commands.push([command, ...args]);
        return { status: 0 };
      },
      listProc: () => processIds,
      readCmdline: () => "browser-native-proxy-engine\0",
      pidAlive: () => now < exitsAt,
      nowMs: () => now,
      sleep: (ms) => { now += ms; },
    });
    assert.deepEqual(leftover, []);
    assert.deepEqual(commands, []);
    assert.equal(now, elapsed);
  });
}

test("guest writer quit gives live writers the full grace before TERM", async () => {
  const commands = [];
  let now = 0;
  const leftover = await quitGuestProfileDiskWriters({
    exec: (command, args = []) => {
      commands.push({ command: [command, ...args], at: now });
      return { status: 0 };
    },
    listProc: () => ["9"],
    readCmdline: () => "browser-native-proxy-engine\0",
    pidAlive: () => now < 4150,
    nowMs: () => now,
    sleep: (ms) => { now += ms; },
  });
  assert.deepEqual(leftover, []);
  assert.deepEqual(commands, [{ command: ["kill", "-TERM", "9"], at: 4000 }]);
  assert.equal(now, 4150);
});

test("guest writer quit signals only writers still alive after grace", async () => {
  const commands = [];
  let now = 0;
  const leftover = await quitGuestProfileDiskWriters({
    exec: (command, args = []) => {
      commands.push({ command: [command, ...args], at: now });
      return { status: 0 };
    },
    listProc: () => ["9", "10"],
    readCmdline: () => "browser-native-proxy-engine\0",
    pidAlive: (pid) => now < (pid === 9 ? 150 : 4150),
    nowMs: () => now,
    sleep: (ms) => { now += ms; },
  });
  assert.deepEqual(leftover, []);
  assert.deepEqual(commands, [{ command: ["kill", "-TERM", "10"], at: 4000 }]);
  assert.equal(now, 4150);
});

test("guest writer quit sends TERM then KILL for leftover pids", async () => {
  const commands = [];
  let now = 0;
  const leftover = await quitGuestProfileDiskWriters({
    exec: (command, args = []) => {
      commands.push({ command: [command, ...args], at: now });
      return { status: 0 };
    },
    listProc: () => ["9"],
    readCmdline: () => "browser-native-proxy-engine\0",
    pidAlive: () => true,
    nowMs: () => now,
    sleep: (ms) => { now += ms; },
  });
  assert.deepEqual(leftover, [9]);
  assert.deepEqual(commands, [
    { command: ["kill", "-TERM", "9"], at: 4000 },
    { command: ["kill", "-KILL", "9"], at: 9000 },
  ]);
  assert.equal(now, 9000);
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

for (const flushed of [false, true]) {
  for (const unmounted of [false, true]) {
    test(`profile durability requires flush (${flushed}) and unmount (${unmounted})`, () => {
      assert.deepEqual(browserProfileDiskDurability({ flushed, unmounted }), {
        ok: flushed && unmounted,
        profile_disk_flushed: flushed,
        profile_disk_unmounted: unmounted,
        leftover_writers_killed: false,
      });
    });
  }
}
