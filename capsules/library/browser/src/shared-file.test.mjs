import assert from 'node:assert/strict';
import test from 'node:test';
import * as shared from './shared-file.js';

test('Library shows the exact local CID with a download control', () => {
  const cid = 'bafybeigdyrzt5sfp7udm7hu76uh7y26nf3w2pz3hftb5xg3mh5g4xq7y5u';
  const markup = shared.sharedFileMarkup(cid);
  assert.ok(markup.includes(`elastos://${cid}`));
  assert.ok(markup.includes(`href="/content/${cid}"`));
  assert.ok(markup.includes(`download="${cid}"`));
  assert.ok(markup.includes('Download file'));
});

test('Library escapes link text and confines downloads to the local gateway', () => {
  const markup = shared.sharedFileMarkup('bad"/><script>');
  assert.ok(!markup.includes('<script>'));
  assert.ok(markup.includes('href="/content/bad%22%2F%3E%3Cscript%3E"'));
});

test('Library uses the post filename and falls back to the CID', () => {
  assert.ok(shared.sharedFileMarkup('cid', 'report.pdf').includes('download="report.pdf"'));
  assert.ok(shared.sharedFileMarkup('cid', '').includes('download="cid"'));
  assert.ok(shared.sharedFileMarkup('cid', 'report".pdf').includes('download="report&quot;.pdf"'));
});
