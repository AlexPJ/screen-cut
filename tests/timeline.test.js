// node --test tests/*.test.js
const test = require("node:test");
const assert = require("node:assert");
const { ranges, imageAt, fmt, parse } = require("../ui/timeline.js");

const images = [{ t_ms: 10000 }, { t_ms: 60000 }, { t_ms: 90000 }];

test("each image lasts until the next one, the last until the end", () => {
  assert.deepStrictEqual(ranges(images, 120000, null), [
    { start: 10000, end: 60000 },
    { start: 60000, end: 90000 },
    { start: 90000, end: 120000 },
  ]);
});

test("max duration cuts long gaps", () => {
  assert.deepStrictEqual(ranges(images, 120000, 20)[0], { start: 10000, end: 30000 });
  assert.deepStrictEqual(ranges(images, 120000, 60)[1], { start: 60000, end: 90000 });
});

test("manual edits win over computed values", () => {
  const edited = [{ t_ms: 10000, start_ms: 0 }, { t_ms: 60000, end_ms: 70000 }];
  assert.deepStrictEqual(ranges(edited, 100000, null), [
    { start: 0, end: 60000 },
    { start: 60000, end: 70000 },
  ]);
});

test("imageAt picks the image valid at a moment", () => {
  const rs = ranges(images, 120000, null);
  assert.strictEqual(imageAt(5000, rs), -1); // antes de la primera captura
  assert.strictEqual(imageAt(10000, rs), 0);
  assert.strictEqual(imageAt(59999, rs), 0);
  assert.strictEqual(imageAt(60000, rs), 1);
  assert.strictEqual(imageAt(119000, rs), 2);
  assert.strictEqual(imageAt(30000, ranges(images, 120000, 10)), -1); // hueco por la duración máxima
});

test("overlapping edits: the later start wins", () => {
  const rs = [{ start: 0, end: 100 }, { start: 50, end: 80 }];
  assert.strictEqual(imageAt(60, rs), 1);
  assert.strictEqual(imageAt(90, rs), 0);
});

test("fmt and parse round-trip", () => {
  assert.strictEqual(fmt(734000), "12:14");
  assert.strictEqual(fmt(3723000), "01:02:03");
  assert.strictEqual(fmt(5000, true), "00:00:05");
  assert.strictEqual(parse("12:14"), 734000);
  assert.strictEqual(parse("1:02:03"), 3723000);
  assert.strictEqual(parse("45"), 45000);
  assert.strictEqual(parse("1:x"), null);
  assert.strictEqual(parse(""), null);
});
