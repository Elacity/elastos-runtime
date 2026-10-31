import assert from 'node:assert/strict';
import test from 'node:test';
import * as shared from './shared-file.js';

test('Library shows the exact local CID with a download control', () => {
  const cid = 'bafybeigdyrzt5sfp7udm7hu76uh7y26nf3w2pz3hftb5xg3mh5g4xq7y5u';
  const markup = shared.sharedFileMarkup(cid);
  assert.ok(markup.includes(`elastos://${cid}`));
  assert.ok(markup.includes('data-shared-download>Download file</button>'));
});

test('Library escapes link text and never links the frame to /content', () => {
  const markup = shared.sharedFileMarkup('bad"/><script>');
  assert.ok(!markup.includes('<script>'));
  assert.ok(!markup.includes('href='));
  assert.ok(!markup.includes('download='));
});
