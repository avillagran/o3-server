// Execute the original lock-verified Playwright keyboard classes, with no browser.
import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
import { createRequire } from 'node:module';
const [artifact, output] = process.argv.slice(2);
if (!artifact || !output) throw Error('Explicit artifact and output directories required');
const manifest = JSON.parse(fs.readFileSync(path.join(artifact, 'artifact.json')));
if (manifest.version !== '1.60.0' || manifest.integrity !== 'sha512-9bW6zvX/m0lEbgTKJ6YppOKx8H3VOPBMOCFh2irXFOT4BbHgrx5hPjwJYLT40Lu+4qtD36qKc/Hn56StUW57IA==') throw Error('Wrong native dependency');
for (const file of manifest.files) {
  const name = path.join(artifact, file.path);
  if (!fs.lstatSync(name).isFile() || crypto.createHash('sha256').update(fs.readFileSync(name)).digest('hex') !== file.sha256) throw Error('Changed artifact');
}
const filename = path.join(artifact, 'package/lib/coreBundle.js');
const original = fs.readFileSync(filename, 'utf8');
const hash = crypto.createHash('sha256').update(original).digest('hex');
if (hash !== 'f74353fcb8e406756a70a6af0dfc4a5069acd577e35ec9d923ccf36ac009c2f5') throw Error('Changed keyboard source');
const marker = '\ninit_coreBundle();\n';
if (original.split(marker).length !== 2) throw Error('Changed bundle boundary');
const source = original.replace(marker, '\n') + '\ninit_input();init_crInput();module.exports.__keyboard={Keyboard,RawKeyboardImpl,usKeyboardLayout,macEditingCommands};\n';
function load(serverMac) {
  const module = { exports: {} };
  const environment = Object.create(process, { platform: { value: serverMac ? 'darwin' : 'linux' } });
  new Function('require','module','exports','__filename','__dirname','process',source)(createRequire(filename),module,module.exports,filename,path.dirname(filename),environment);
  return module.exports.__keyboard;
}
const first = load(false);
const platformStart = original.indexOf('      _platform() {');
const platformEnd = original.indexOf('      isClank()', platformStart);
if (platformStart < 0 || platformEnd < platformStart) throw Error('Changed native platform boundary');
const nativePlatform = new Function('userAgent', 'const browser = {_userAgent:userAgent,' + original.slice(platformStart,platformEnd).trim() + '};return browser._platform();');
const platformCases = ['','Windows','Macintosh','Windows Macintosh','HeadlessChrome Linux','macintosh','iPhone like Mac OS X'].map(userAgent=>({userAgent,platform:nativePlatform(userAgent)}));
const constants = { version: manifest.version, sourceSha256: hash, layout: Object.fromEntries(first.usKeyboardLayout), macCommands: first.macEditingCommands };
const keys = [...first.usKeyboardLayout.keys()];
const cases = [];
for (const serverMac of [false,true]) for (const browserMac of [false,true]) {
  const {Keyboard,RawKeyboardImpl} = load(serverMac);
  const programs = keys.map(key=>[{key}]);
  for (const key of ['Control+v','Control+c','ControlOrMeta+A','ControlOrMeta+Shift+Z','Control++','Shift++','+','Control+Control+v','Control+NoSuchKey','NoSuchKey','', 'Shift+Digit1', 'Alt+ArrowLeft', 'Meta+ArrowLeft','Control+Alt+Delete']) programs.push([{key}]);
  programs.push([{key:'Control+NoSuchKey'},{key:'v'},{key:'Control'},{key:'v'}]);
  programs.push([{key:'Control+v', failAt:0},{key:'v'},{key:'Control'}]);
  programs.push([{key:'Shift+KeyA', failAt:1},{key:'KeyA'},{key:'Shift'}]);
  for (const program of programs) {
    const events = []; let failAt;
    const client = {send:async(method,params)=>{events.push({method,params});if(events.length-1===failAt)throw Error('fixture-send-failed');}};
    const keyboard = new Keyboard(new RawKeyboardImpl(client,browserMac,{cancelDrag:async()=>false}),{});
    const progress = {race:value=>value,wait:async()=>{}};
    const operations = [];
    for (const input of program) {
      failAt = input.failAt === undefined ? undefined : events.length + input.failAt;
      let error = null;
      try {await keyboard.press(progress,input.key);} catch (value) {error=value.message;}
      operations.push({input,error,events:events.splice(0),modifiers:[...keyboard._pressedModifiers],keys:[...keyboard._pressedKeys]});
    }
    cases.push({serverMac,browserMac,operations});
  }
}
fs.writeFileSync(path.join(output,'keyboard-data.json'),JSON.stringify(constants,null,2)+'\n',{flag:'wx'});
fs.writeFileSync(path.join(output,'keyboard-cases.json'),JSON.stringify({version:manifest.version,sourceSha256:hash,platformCases,cases})+'\n',{flag:'wx'});
for (const name of ['LICENSE','NOTICE']) fs.copyFileSync(path.join(artifact,'package',name),path.join(output,name),fs.constants.COPYFILE_EXCL);
console.log(JSON.stringify({cases:cases.length,layoutEntries:keys.length,sourceSha256:hash}));
