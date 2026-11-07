#!/usr/bin/env node
// Execute whole native VCS orchestration with an inert process boundary; no child is launched.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [staged,output]=process.argv.slice(2),revision='83a3674ffd84ffb33de768210c806cb1ac53fd80';
const provenance=JSON.parse(fs.readFileSync(staged+'/SOURCE.json','utf8'));
const hash=text=>crypto.createHash('sha256').update(text).digest('hex');
if(provenance.revision!==revision)throw Error('Wrong native pin');
for(const [file,digest]of Object.entries(provenance.files))if(hash(fs.readFileSync(staged+'/'+file))!==digest)throw Error('Changed source '+file);
const require=createRequire(staged+'/package.json'),load=name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
const [Effect,Schema,Context,Semaphore,Match,Schedule,Duration]=await Promise.all(['Effect','Schema','Context','Semaphore','Match','Schedule','Duration'].map(load));
const C=await import(pathToFileURL(staged+'/packages/contracts/src/index.ts').href);
const HostProcess=await import(pathToFileURL(staged+'/packages/shared/src/HostProcess.ts').href);
if(!Schema.is(C.ProviderInstanceId)('codex'))throw Error('Canonical Effect schema identity required');
if(JSON.parse(fs.readFileSync(require.resolve('effect/package.json'))).version!=='4.1.2')throw Error('Wrong Effect');
const schemaCases=[];
for(const input of [{},{sidebarAutoSettleAfterDays:null},{sidebarAutoSettleAfterDays:3},{sidebarAutoSettleAfterDays:7},{sidebarAutoSettleAfterDays:14},{sidebarAutoSettleAfterDays:0},{sidebarAutoSettleAfterDays:'7'}]){
 try{const decoded=await Effect.runPromise(Schema.decodeUnknownEffect(C.ServerSettings)(input));schemaCases.push({input,value:decoded.sidebarAutoSettleAfterDays});}
 catch{schemaCases.push({input,invalid:true});}
}
const file='apps/server/src/vcs/VcsProcess.ts',source=fs.readFileSync(staged+'/'+file,'utf8');
const body=source.slice(source.indexOf('const DEFAULT_TIMEOUT_MS'),source.indexOf('\nexport const layer =')).replaceAll('export const','const');
if(!body.includes('const make = Effect.gen'))throw Error('Native body boundary moved');
class Runner extends Context.Service()('fixture/ProcessRunner'){}
const VcsProcess={of:value=>value};
const globals={Effect,Semaphore,Match,Schedule,Duration,HostProcess,ProcessRunner:{ProcessRunner:Runner},VcsProcess,withMetrics:()=>effect=>effect,gitCommandsTotal:{},gitCommandDuration:{},...C};
const make=new Function(...Object.keys(globals),stripTypeScriptTypes(body,{mode:'strip'})+'\nreturn make;')(...Object.values(globals));
const vcsCases=[];
for(const platform of ['linux','darwin'])for(const command of ['git','gh','/private/git'])for(const env of [{},{LANGUAGE:'fr'},{LC_CTYPE:'de_DE.UTF-8',LANG:'es_ES.UTF-8'},{LANGUAGE:'',LC_ALL:'C.UTF-8'}]){
 const calls=[];
 const service=await Effect.runPromise(make.pipe(Effect.provideService(Runner,{run:input=>{calls.push(input);return Effect.succeed({code:0,stdout:'',stderr:'',stdoutTruncated:false,stderrTruncated:false});}}),Effect.provideService(HostProcess.Platform,platform)));
 await Effect.runPromise(service.run({operation:'fixture.locale',command,args:['status'],cwd:'/fixture',env}));
 vcsCases.push({platform,command,env,observed:calls.map(call=>({command:call.command,env:call.env}))});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources:provenance.files,schemaCases,vcsCases,boundaries:['Whole VcsProcess.make and helpers from native source','Actual Effect/Schema/HostProcess/contracts; canonical identity checked','ProcessRunner is an inert capture boundary; metrics instrumentation is identity','No Pi runtime path is implemented in O3; source atomic-write delta is documented only']},null,2)+'\n');
console.log('Native4314: '+schemaCases.length+' settings cases, '+vcsCases.length+' whole VCS locale traces');
