// Phone layout checks shared by the capsule product-layout smokes.
//
// On a phone stage the shell posts elastos:shell-layout and the shared theme
// runtime sets html[data-el-form-factor="phone"]; fixture hosts without the
// shell set that attribute by hand through setPhoneFormFactor.

export const PHONE_VIEWPORT = { width: 390, height: 844 };

// Longer than --el-drawer-ms (420ms) so the slide has settled.
export const DRAWER_SETTLE_MS = 500;

export async function setPhoneFormFactor(target, isPhone = true) {
  await target.evaluate((next) => {
    if (next) {
      document.documentElement.setAttribute("data-el-form-factor", "phone");
    } else {
      document.documentElement.removeAttribute("data-el-form-factor");
    }
  }, isPhone);
}

function check(condition, message, detail) {
  if (!condition) {
    throw new Error(`${message}: ${JSON.stringify(detail)}`);
  }
}

function readDrawer(target, drawer, room) {
  return target.evaluate(({ drawer, room }) => {
    const drawerNode = document.querySelector(drawer);
    const roomNode = document.querySelector(room);
    const toggle = document.querySelector(`[data-el-drawer-toggle="${drawerNode.id}"]`);
    const toggleRect = toggle.getBoundingClientRect();
    const roomRect = roomNode.getBoundingClientRect();
    return {
      drawerRight: Math.round(drawerNode.getBoundingClientRect().right),
      drawerInert: drawerNode.inert,
      roomLeft: Math.round(roomRect.left),
      roomTop: Math.round(roomRect.top),
      toggle: {
        width: Math.round(toggleRect.width),
        height: Math.round(toggleRect.height),
        expanded: toggle.getAttribute("aria-expanded"),
      },
    };
  }, { drawer, room });
}

// Push drawer contract: content first with the sidebar off screen and inert; a
// 44 px toggle slides it in and pushes the room aside; picking an item closes
// it again.
export async function assertPhoneDrawer(page, target, { label, drawer, room, closeTarget, screenshot }) {
  const closed = await readDrawer(target, drawer, room);
  check(closed.drawerRight <= 0 && closed.drawerInert && closed.roomLeft === 0 && closed.roomTop === 0,
    `${label} phone: content must fill the stage with the sidebar off screen and inert`, closed);
  check(closed.toggle.width >= 44 && closed.toggle.height >= 44 && closed.toggle.expanded === "false",
    `${label} phone: sidebar toggle must be a 44 px collapsed control`, closed);

  await target.locator(`[data-el-drawer-toggle]`).first().click();
  await page.waitForTimeout(DRAWER_SETTLE_MS);
  const opened = await readDrawer(target, drawer, room);
  if (screenshot) {
    await page.screenshot({ path: screenshot });
  }
  check(opened.drawerRight > 200 && !opened.drawerInert && opened.roomLeft >= opened.drawerRight - 2 && opened.toggle.expanded === "true",
    `${label} phone: the toggle must slide the sidebar in and push the content aside`, opened);

  await target.locator(closeTarget).first().click();
  await page.waitForTimeout(DRAWER_SETTLE_MS);
  const picked = await readDrawer(target, drawer, room);
  check(picked.drawerRight <= 0 && picked.roomLeft === 0 && picked.toggle.expanded === "false",
    `${label} phone: picking an item must close the sidebar`, picked);
}
