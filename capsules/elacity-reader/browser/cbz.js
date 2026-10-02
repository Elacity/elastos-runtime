// Comic books are archives of numbered images. The order the archive lists
// them in is not the reading order, so the pages are sorted the way a person
// numbers them: page2 before page10.

import { listEntries, readEntry } from "./zip.js";

const PAGE_TYPES = new Map([
  ["avif", "image/avif"],
  ["bmp", "image/bmp"],
  ["gif", "image/gif"],
  ["jpeg", "image/jpeg"],
  ["jpg", "image/jpeg"],
  ["png", "image/png"],
  ["webp", "image/webp"],
]);

const NO_PAGES = "This comic has no pages.";
const NO_SUCH_PAGE = "This comic does not have that page.";

function fail(message) {
  throw new Error(message);
}

/** Returns the image type a page name carries, or an empty string for anything else. */
export function pageContentType(name) {
  if (typeof name !== "string") return "";
  const dot = name.lastIndexOf(".");
  if (dot < 0) return "";
  return PAGE_TYPES.get(name.slice(dot + 1).toLowerCase()) || "";
}

function isHidden(name) {
  if (name.startsWith("__MACOSX/")) return true;
  return name.split("/").some((segment) => segment.startsWith("."));
}

function runsOf(value) {
  return value.match(/\d+|\D+/g) || [];
}

/**
 * Orders two names the way a reader numbers pages: digit runs compare by value,
 * everything else compares as text with case ignored first so `p8` sorts before
 * `P9`. Equal numbers with different padding keep a defined order rather than
 * an arbitrary one.
 */
export function naturalCompare(left, right) {
  const leftRuns = runsOf(String(left));
  const rightRuns = runsOf(String(right));
  const shared = Math.min(leftRuns.length, rightRuns.length);
  // Case and zero padding only ever break a tie, so they are recorded and
  // applied last rather than cutting the run-by-run comparison short.
  let tieBreak = 0;
  for (let index = 0; index < shared; index += 1) {
    const a = leftRuns[index];
    const b = rightRuns[index];
    if (/^\d/.test(a) && /^\d/.test(b)) {
      // Compared as digits rather than as numbers. A file name can carry any
      // number of digits, and converting them first would silently lose every
      // digit past the sixteenth and give up altogether past a few hundred,
      // which would make two pages compare equal that are not, and leave the
      // reading order to whichever sort the browser happens to use. Once the
      // leading zeros are off, the longer run is the larger number, and runs
      // of the same length compare left to right.
      const digitsA = a.replace(/^0+(?=\d)/, "");
      const digitsB = b.replace(/^0+(?=\d)/, "");
      if (digitsA.length !== digitsB.length) return digitsA.length < digitsB.length ? -1 : 1;
      if (digitsA !== digitsB) return digitsA < digitsB ? -1 : 1;
    } else {
      const foldedA = a.toLowerCase();
      const foldedB = b.toLowerCase();
      if (foldedA !== foldedB) return foldedA < foldedB ? -1 : 1;
    }
    if (tieBreak === 0 && a !== b) tieBreak = a < b ? -1 : 1;
  }
  if (leftRuns.length !== rightRuns.length) return leftRuns.length < rightRuns.length ? -1 : 1;
  return tieBreak;
}

/** Keeps the image entries of an archive and puts them in reading order. */
export function listPages(entries) {
  return (Array.isArray(entries) ? entries : [])
    .filter(
      (entry) =>
        entry &&
        typeof entry.name === "string" &&
        !entry.isDirectory &&
        !isHidden(entry.name) &&
        pageContentType(entry.name) !== "",
    )
    .sort((left, right) => naturalCompare(left.name, right.name));
}

/**
 * Tracks which page is on screen. `next` and `previous` stop at the ends;
 * `goTo` takes an explicit destination and refuses one the comic does not have.
 */
export function createPager(count) {
  if (!Number.isSafeInteger(count) || count < 1) fail(NO_PAGES);
  let index = 0;
  function goTo(target) {
    if (!Number.isSafeInteger(target) || target < 0 || target >= count) fail(NO_SUCH_PAGE);
    index = target;
    return index;
  }
  return {
    get count() {
      return count;
    },
    get index() {
      return index;
    },
    get hasPrevious() {
      return index > 0;
    },
    get hasNext() {
      return index < count - 1;
    },
    goTo,
    next() {
      return index < count - 1 ? goTo(index + 1) : index;
    },
    previous() {
      return index > 0 ? goTo(index - 1) : index;
    },
  };
}

/**
 * Opens a comic and returns its pages in reading order.
 *
 * @param {Uint8Array} bytes
 * @returns {Promise<{
 *   pageCount: number,
 *   pages: { name: string, contentType: string }[],
 *   pager: ReturnType<typeof createPager>,
 *   readPage: (index: number) => Promise<Uint8Array>,
 * }>}
 */
export async function openComic(bytes) {
  const pages = listPages(listEntries(bytes));
  if (pages.length === 0) fail(NO_PAGES);
  return {
    pageCount: pages.length,
    pages: pages.map((page) => ({ name: page.name, contentType: pageContentType(page.name) })),
    pager: createPager(pages.length),
    async readPage(index) {
      if (!Number.isSafeInteger(index) || index < 0 || index >= pages.length) fail(NO_SUCH_PAGE);
      return readEntry(bytes, pages[index]);
    },
  };
}
