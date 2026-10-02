import assert from "node:assert/strict";
import test from "node:test";

import { createPager, listPages, naturalCompare, openComic, pageContentType } from "./cbz.js";
import { buildStoredZip, buildZip, text } from "./zip-fixture.test-helper.mjs";

const decoder = new TextDecoder();

function entriesOf(names) {
  return names.map((name) => ({ name, isDirectory: name.endsWith("/") }));
}

function namesOf(entries) {
  return entries.map((entry) => entry.name);
}

async function rejects(run, expected) {
  await assert.rejects(async () => run(), (error) => {
    assert.ok(error instanceof Error, "a plain Error must reach the reader");
    assert.match(error.message, expected);
    return true;
  });
}

test("pages sort in natural order rather than by code point", () => {
  assert.deepEqual(
    namesOf(listPages(entriesOf(["page10.jpg", "page2.jpg", "page1.jpg"]))),
    ["page1.jpg", "page2.jpg", "page10.jpg"],
  );
});

test("natural order handles padding, depth and mixed case", () => {
  assert.deepEqual(
    namesOf(
      listPages(
        entriesOf([
          "ch2/p1.png",
          "ch10/p1.png",
          "ch1/p007.png",
          "ch1/p8.png",
          "ch1/P9.png",
        ]),
      ),
    ),
    ["ch1/p007.png", "ch1/p8.png", "ch1/P9.png", "ch2/p1.png", "ch10/p1.png"],
  );
  assert.ok(naturalCompare("a2", "a10") < 0);
  assert.ok(naturalCompare("a10", "a2") > 0);
  assert.equal(naturalCompare("a2", "a2"), 0);
  // Equal numeric value with different padding stays in a stable, defined order.
  assert.ok(naturalCompare("a02", "a2") < 0);
});

test("naturalCompare stays consistent on page numbers longer than a number can hold", () => {
  // A comic can carry any digits at all in a file name. Comparing digit runs as
  // numbers loses every digit past the 16th and gives up entirely past a few
  // hundred, which would make the order depend on the sort the engine happens
  // to use. Digits are compared as digits: more of them is larger, and equal
  // counts compare left to right.
  const long = (suffix) => `p${"9".repeat(30)}${suffix}.png`;
  assert.ok(naturalCompare(long("1"), long("2")) < 0);
  assert.ok(naturalCompare(long("2"), long("1")) > 0);
  assert.equal(naturalCompare(long("1"), long("1")), 0);
  const huge = (digits) => `p${"1".repeat(digits)}.png`;
  assert.ok(naturalCompare(huge(400), huge(401)) < 0);
  assert.ok(naturalCompare(huge(401), huge(400)) > 0);
  assert.ok(naturalCompare(`p${"0".repeat(400)}5.png`, `p${"0".repeat(9)}6.png`) < 0);

  // Consistency: sorting the same set from two different starting orders must
  // land on the same reading order.
  const names = [long("3"), long("1"), huge(400), long("2"), huge(401), "p1.png"];
  const forward = [...names].sort(naturalCompare);
  const backward = [...names].reverse().sort(naturalCompare);
  assert.deepEqual(forward, backward);
});

test("listPages keeps only the image entries a comic can show", () => {
  assert.deepEqual(
    namesOf(
      listPages(
        entriesOf([
          "cover.jpg",
          "pages/",
          "pages/01.PNG",
          "pages/02.webp",
          "pages/03.gif",
          "pages/04.avif",
          "notes.txt",
          "ComicInfo.xml",
          "__MACOSX/pages/._01.png",
          "pages/.hidden.png",
          "pages/thumbs.db",
        ]),
      ),
    ),
    ["cover.jpg", "pages/01.PNG", "pages/02.webp", "pages/03.gif", "pages/04.avif"],
  );
});

test("pageContentType names the image type each page carries", () => {
  assert.equal(pageContentType("a/b.JPG"), "image/jpeg");
  assert.equal(pageContentType("a/b.jpeg"), "image/jpeg");
  assert.equal(pageContentType("a/b.png"), "image/png");
  assert.equal(pageContentType("a/b.gif"), "image/gif");
  assert.equal(pageContentType("a/b.webp"), "image/webp");
  assert.equal(pageContentType("a/b.avif"), "image/avif");
  assert.equal(pageContentType("a/b.bmp"), "image/bmp");
  assert.equal(pageContentType("a/b.txt"), "");
});

test("openComic exposes the pages in reading order and reads their bytes", async () => {
  const bytes = buildStoredZip({
    "page10.jpg": "tenth page",
    "page2.jpg": "second page",
    "page1.jpg": "first page",
    "ComicInfo.xml": "<ComicInfo/>",
  });
  const comic = await openComic(bytes);
  assert.equal(comic.pageCount, 3);
  assert.deepEqual(
    comic.pages.map((page) => page.name),
    ["page1.jpg", "page2.jpg", "page10.jpg"],
  );
  assert.deepEqual(
    comic.pages.map((page) => page.contentType),
    ["image/jpeg", "image/jpeg", "image/jpeg"],
  );
  assert.equal(decoder.decode(await comic.readPage(0)), "first page");
  assert.equal(decoder.decode(await comic.readPage(2)), "tenth page");
});

test("openComic refuses a page index outside the comic", async () => {
  const comic = await openComic(buildStoredZip({ "p1.png": "one" }));
  for (const index of [-1, 1, 1.5, Number.NaN, "0"]) {
    await rejects(() => comic.readPage(index), /page/i);
  }
});

test("openComic refuses an archive with no pages", async () => {
  await rejects(() => openComic(buildStoredZip({ "notes.txt": "no images here" })), /no pages/i);
});

test("openComic passes an unreadable archive straight through", async () => {
  await rejects(
    () => openComic(buildZip([{ name: "p1.png", data: text("x"), flags: 0x0001 }])),
    /password protected/i,
  );
  await rejects(() => openComic(text("not an archive")), /not a readable archive/i);
});

test("the pager walks pages and stops at both ends", () => {
  const pager = createPager(3);
  assert.equal(pager.count, 3);
  assert.equal(pager.index, 0);
  assert.equal(pager.hasPrevious, false);
  assert.equal(pager.hasNext, true);
  assert.equal(pager.previous(), 0);
  assert.equal(pager.next(), 1);
  assert.equal(pager.next(), 2);
  assert.equal(pager.hasNext, false);
  assert.equal(pager.next(), 2);
  assert.equal(pager.previous(), 1);
  assert.equal(pager.goTo(0), 0);
  assert.equal(pager.hasPrevious, false);
});

test("the pager refuses a destination it cannot show", () => {
  const pager = createPager(2);
  for (const target of [-5, 9, 1.5, Number.NaN, "1", null]) {
    assert.throws(() => pager.goTo(target), /page/i);
  }
  assert.equal(pager.index, 0);
  assert.throws(() => createPager(0), /no pages/i);
  assert.throws(() => createPager(-1), /no pages/i);
});
