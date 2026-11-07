// Read exact pinned source from a retained git snapshot or a verified staged manifest.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
export function pinnedSource(checkout,revision,sources){
 const manifestPath=path.join(checkout,'SOURCE.json');
 const manifest=fs.existsSync(manifestPath)?JSON.parse(fs.readFileSync(manifestPath,'utf8')):null;
 if(manifest&&manifest.revision!==revision)throw Error('Staged native revision mismatch');
 return file=>{
  if(path.isAbsolute(file)||file.split('/').includes('..'))throw Error('Invalid native source path');
  const value=manifest?fs.readFileSync(path.join(checkout,file),'utf8'):execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
  const hash=crypto.createHash('sha256').update(value).digest('hex');
  if(manifest&&manifest.files[file]!==hash)throw Error('Staged native source mismatch: '+file);
  sources[file]=hash;return value;
 };
}
