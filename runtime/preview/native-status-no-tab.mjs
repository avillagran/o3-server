// Complete pinned original no-tab formatter; no browser service is launched.
import fs from 'node:fs';
import {createHash} from 'node:crypto';
import {stripTypeScriptTypes} from 'node:module';
import {compileFunction} from 'node:vm';
const SOURCE_SHA = 'e8b025a1c7de82ba8089c5fa7d4439ee69e1d19a74af7657a26a419a51cdfab6';
const digest = value => createHash('sha256').update(value).digest('hex');
// Private no-tab projection only. The complete formatter is retained, but no
// caller can pass a tab, Page, executable source, or profile authority here.
export function originalNoTabStatus({serverBrowserSource}) {
  if (typeof serverBrowserSource !== 'string' || digest(serverBrowserSource) !== SOURCE_SHA)
    throw Error('Original status source integrity unavailable');
  const manifest = JSON.parse(fs.readFileSync(new URL('./status-no-tab-native-manifest.json', import.meta.url), 'utf8'));
  if (manifest.sha256 !== SOURCE_SHA || manifest.revision !== 'a9e709f15f825cacac1b6d665fa1920c328d68b7')
    throw Error('Original status closure manifest unavailable');
  const closures = ['profileStatus', 'statusWithTitle'].map(name => {
    const row = manifest.methods[name];
    if (!row || serverBrowserSource.split(row.start).length !== 2) throw Error('Original status closure unavailable');
    const at = serverBrowserSource.indexOf(row.start), until = serverBrowserSource.indexOf(row.end, at);
    const complete = serverBrowserSource.slice(at, until);
    if (until <= at || digest(complete) !== row.sha256) throw Error('Original status closure integrity unavailable');
    return complete;
  });
  const source = stripTypeScriptTypes(closures.join('\n'), {mode:'strip'});
  const format = compileFunction(`${source}\nreturn statusWithTitle(undefined);`,
    ['reportedProfiles', 'BUILT_IN_BROWSER_PROFILES'], {filename:'pinned-ServerBrowser-no-tab-status.ts'});
  return catalog => {
    if (!catalog || typeof catalog !== 'object' || Array.isArray(catalog) ||
        !Object.keys(catalog).every(key => ['profiles','defaultProfileId'].includes(key)) ||
        !Array.isArray(catalog.profiles) || catalog.profiles.some(profile =>
          !profile || typeof profile !== 'object' || Array.isArray(profile) ||
          Object.keys(profile).sort().join(',') !== 'id,kind,name' ||
          typeof profile.id !== 'string' || typeof profile.name !== 'string' ||
          !['persistent','incognito'].includes(profile.kind)) ||
        ('defaultProfileId' in catalog && typeof catalog.defaultProfileId !== 'string'))
      throw Error('Owned status catalog unavailable');
    return format('defaultProfileId' in catalog ? catalog : undefined, catalog.profiles);
  };
}
