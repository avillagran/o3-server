// Whole pinned native contract modules and unmodified catalog/report-token
// closure. No browser, files in a profile, RPC server or provider is started.
import assert from 'node:assert/strict';
import fs from 'node:fs';import vm from 'node:vm';import path from 'node:path';
import {createHash} from 'node:crypto';
import {createRequire,stripTypeScriptTypes} from 'node:module';import {pathToFileURL} from 'node:url';
const revision='83a3674ffd84ffb33de768210c806cb1ac53fd80';
const pins={
 'packages/contracts/src/baseSchemas.ts':'edd4d2c73cc79ce5e50bffd35b9c861fe091de23a1ac792db864d3b71b09a7af',
 'packages/contracts/src/browserProfile.ts':'a8995cb24686afddd7dc66a85b2c61019d1ffbd551dac26864fffbc13e9f10e2',
 'packages/contracts/src/preview.ts':'d93c13bf3c0510629d61cce4249192cc083b3849d00c7e9f2c4abd70b1d4fb3d',
 'apps/server/src/preview/ServerBrowser.ts':'e8b025a1c7de82ba8089c5fa7d4439ee69e1d19a74af7657a26a419a51cdfab6',
};
const sources=new Map();for(const [name,pin]of Object.entries(pins)){
 const file='/toolkit-native/'+name;const identity=fs.lstatSync(file);assert(identity.isFile()&&!identity.isSymbolicLink());
 const source=fs.readFileSync(file,'utf8');assert.equal(createHash('sha256').update(source).digest('hex'),pin,name);sources.set(name,source);
}
const require=createRequire('/deps/package.json');const load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),Scope=await load('effect/Scope'),Exit=await load('effect/Exit'),Schema=await load('effect/Schema');
const modules=new Map(),synthetics=new Map();
function module(name){if(modules.has(name))return modules.get(name);const result=new vm.SourceTextModule(stripTypeScriptTypes(sources.get(name),{mode:'strip'}),{identifier:name});modules.set(name,result);return result;}
async function linker(specifier,reference){
 if(specifier.startsWith('.'))return module(path.posix.normalize(path.posix.join(path.posix.dirname(reference.identifier),specifier)));
 if(synthetics.has(specifier))return synthetics.get(specifier);
 const values=await load(specifier);const result=new vm.SyntheticModule(Object.keys(values),function(){for(const[key,value]of Object.entries(values))this.setExport(key,value);});synthetics.set(specifier,result);return result;
}
const browser=sources.get('apps/server/src/preview/ServerBrowser.ts');
const start='  const reportsByClient = ',end='  const resolveOpenProfile = ';
assert.equal(browser.split(start).length,2);const at=browser.indexOf(start),until=browser.indexOf(end,at);assert(until>at);
const fragment=browser.slice(at,until);
const wrapper=`import * as Effect from 'effect/Effect';
import {BUILT_IN_BROWSER_PROFILES,DEFAULT_BROWSER_PROFILE_ID,resolveBrowserProfiles,findBrowserProfile} from './browserProfile.ts';
import {DEFAULT_BROWSER_RECORDING_FRAME_RATE} from './preview.ts';
export function create(){${fragment}\nreturn {profileReporter,profileStatus,rate:()=>recordingFrameRate};}`;
const catalogModule=new vm.SourceTextModule(stripTypeScriptTypes(wrapper,{mode:'strip'}),{identifier:'packages/contracts/src/catalog-fixture.ts'});
await catalogModule.link(linker);await catalogModule.evaluate();
const contract=modules.get('packages/contracts/src/preview.ts').namespace;
const profile=(id,name=id,kind='persistent')=>({id,name,kind});
const report=(profiles,defaultProfileId='default',recordingFrameRate)=>({profiles,defaultProfileId,...(recordingFrameRate===undefined?{}:{recordingFrameRate})});
const scenarios=[
 {name:'unreported-acquire-retire',steps:[['acquire','a'],['retire','a']]},
 {name:'union-shadow-update-retire-last',steps:[['acquire','a'],['acquire','b'],['report','a',report([profile('a'),profile('x','Old X')],'a',60)],['report','b',report([profile('b'),profile('x','New X')],'b')],['report','a',report([profile('c'),profile('x','Latest X')],'c')],['retire','a'],['retire','b'],['acquire','empty'],['report','empty',report([],'unknown')],['retire','empty']]},
 {name:'builtin-duplicate-normalization-case-sensitive-default',steps:[['acquire','a'],['report','a',report([profile('incognito','Wrong'),profile('default','Wrong default'),profile('x','First','incognito'),profile('x','Last')],'X')],['report','a',report([profile('custom','Private label','incognito')],'custom',30)],['retire','a']]},
 {name:'rate-independent-of-reporter-retirement',steps:[['acquire','a'],['acquire','b'],['report','a',report([profile('a')],'a',60)],['report','b',report([profile('b')],'b',30)],['retire','b'],['retire','a']]},
];
const traces=[];
for(const scenario of scenarios){
 const catalog=catalogModule.namespace.create(),tokens=new Map();const rows=[{step:['initial'],status:catalog.profileStatus(),recordingFrameRate:catalog.rate()}];
 for(const step of scenario.steps){
  const [action,id,input]=step;
  if(action==='acquire'){
   const scope=await Effect.runPromise(Scope.make());
   const reporter=await Effect.runPromise(catalog.profileReporter.pipe(Effect.provideService(Scope.Scope,scope)));tokens.set(id,{scope,reporter});
  }else if(action==='report'){
   const decoded=Schema.decodeUnknownSync(contract.PreviewReportProfilesInput)(input);
   await Effect.runPromise(tokens.get(id).reporter(decoded));
  }else {await Effect.runPromise(Scope.close(tokens.get(id).scope,Exit.void));tokens.delete(id);}
  rows.push({step,status:catalog.profileStatus(),recordingFrameRate:catalog.rate()});
 }
 assert.equal(tokens.size,0);traces.push({name:scenario.name,rows});
}
const corpus=[
 report([]),report([profile(' \uFEFFx\uFEFF ',' \uFEFFName ')] ,' x ',60.0),
 report([profile('\u0085','Name')]),report([profile('x','\u0085')]),
 report([profile('x','😀'.repeat(24))]),report([profile('x','😀'.repeat(25))]),
 report([profile('x'.repeat(64))]),report([profile('x'.repeat(65))]),
 report([profile('default','Override')]),report([profile('x','Private','incognito')]),
 report(Array.from({length:24},()=>profile('x'))),report(Array.from({length:25},()=>profile('x'))),
 report([], 'default',null),report([], 'default',29),report([], 'default',60.5),report([], 'default','30'),
 {...report([]),ignored:'stripped'},report([{...profile('x'),ignored:true}]),
 report([profile('\uD800','Surrogate ID')]),report([profile('x','\uDFFF')]),
];
const schemaCases=corpus.map(input=>{try{return {input,accepted:true,decoded:Schema.decodeUnknownSync(contract.PreviewReportProfilesInput)(input)};}catch{return {input,accepted:false};}});
fs.writeFileSync('/out/browser-profile-catalog-native.json',JSON.stringify({complete:true,revision,sourceSha256:pins,catalogFragmentSha256:createHash('sha256').update(fragment).digest('hex'),wholeContractModules:true,unmodifiedCatalogAndTokenFinalizers:true,traces,schemaCases,qualification:'Original complete catalog closure and profile/report schemas in actual Effect4.1.2; client tokens use real Effect Scope acquisition/finalization. No O3 Rust execution, RPC/GUI client, browser profile filesystem or status activation. Lone-surrogate native schema cases document the existing generic Rust Value ingress limitation.'},null,2)+'\n');
console.log('NATIVE_BROWSER_PROFILE_CATALOG_PASS');
