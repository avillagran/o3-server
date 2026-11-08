// Source-only test preparation. Run only in the reviewed isolated Node fixture.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {originalNoTabStatus} from './native-status-no-tab.mjs';

const root = process.env.O3_STATUS_SOURCE_ROOT;
if (!root || !path.isAbsolute(root)) throw Error('An admitted absolute source fixture is required');
const serverBrowserSource = fs.readFileSync(path.join(root, 'apps/server/src/preview/ServerBrowser.ts'), 'utf8');
const format = originalNoTabStatus({serverBrowserSource});
const profiles = [
  {id:'default', name:'Default', kind:'persistent'},
  {id:'incognito', name:'Incognito', kind:'incognito'},
];
const absent = {available:false, visible:false, tabId:null, url:null, title:null, loading:false};

test('whole original no-tab formatter uses actual fallback catalog and omits unreported default', async () => {
  assert.deepEqual(await format({profiles}), {...absent, profiles:[
    {id:'default', name:'Default'}, {id:'incognito', name:'Incognito', incognito:true},
  ]});
});
test('whole original profile closure preserves reported order, duplicate IDs, Unicode and default', async () => {
  const profiles = [
    {id:'same', name:'Trabajo 🚀', kind:'incognito'},
    {id:'same', name:'第二', kind:'persistent'},
  ];
  assert.deepEqual(await format({profiles, defaultProfileId:'same'}), {...absent,
    profiles:[{id:'same', name:'Trabajo 🚀', incognito:true}, {id:'same', name:'第二'}],
    defaultProfileId:'same',
  });
});
test('formatter rejects incomplete catalogs and authority-shaped extra fields', () => {
  for (const catalog of [null, {}, {profiles:null}, {profiles, tab:{}},
    {profiles, defaultProfileId:null}, {profiles:[{id:'x', name:'X', kind:'unknown'}]}]) {
    assert.throws(() => format(catalog), /Owned status catalog unavailable/);
  }
});
test('complete original source integrity is mandatory', () => {
  assert.throws(() => originalNoTabStatus({serverBrowserSource:serverBrowserSource+'\n'}), /integrity/);
});
