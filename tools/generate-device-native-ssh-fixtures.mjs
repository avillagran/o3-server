#!/usr/bin/env node
// Execute the complete native SSH host factory with fake child handles; scripts are never run.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,dependencies]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-device-native-ssh-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const directory=path.dirname(output);fs.mkdirSync(directory,{recursive:true});
const sources={},loaded=new Map();
function module(file){
 if(loaded.has(file))return loaded.get(file);
 const target=path.join(directory,crypto.createHash('sha256').update(file).digest('hex')+(file.endsWith('.json')?'.json':'.mjs'));
 loaded.set(file,target);
 const source=execFileSync('git',['-c','safe.directory='+checkout,'-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
 sources[file]=crypto.createHash('sha256').update(source).digest('hex');
 if(file.endsWith('.json')){fs.writeFileSync(target,source);return target;}
 const code=stripTypeScriptTypes(source).replace(/from\s+(["'])([^"']+)\1/g,(all,quote,specifier)=>{
  let destination;
  if(specifier==='@t3tools/contracts')destination=contracts+'/src/index.ts';
  else if(specifier.startsWith('effect/'))destination=require.resolve(specifier);
  else if((specifier.startsWith('@effect/')||specifier.startsWith('@noble/')))destination=createRequire(dependencies+'/package.json').resolve(specifier);
  else if(specifier.startsWith('./')||specifier.startsWith('../'))destination=module(path.posix.normalize(path.posix.join(path.posix.dirname(file),specifier)));
  else if(specifier.startsWith('@t3tools/')){
   const [,pkg,entry]=specifier.match(/^@t3tools\/([^/]+)(?:\/(.+))?$/)??[];
   if(!pkg)throw Error('Unhandled package '+specifier);
   destination=module(`packages/${pkg}/src/${entry??'index'}.ts`);
  }else if(specifier==='diff') {
   if(!dependencies)throw Error('Pinned diff dependency directory required');
   const folder=path.join(dependencies,'node_modules/diff');const metadata=JSON.parse(fs.readFileSync(path.join(folder,'package.json'),'utf8'));if(metadata.version!=='8.0.3')throw Error('diff8.0.3 required');destination=path.join(folder,metadata.module??metadata.main);
  }else return all;
  return 'from '+JSON.stringify(pathToFileURL(destination).href);
 });
 fs.writeFileSync(target,code);return target;
}
const native=await import(pathToFileURL(module('apps/server/src/device/SshDeviceHost.ts')).href);
const script=await import(pathToFileURL(module('apps/server/src/device/sshDeviceScript.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Layer=await import(pathToFileURL(require.resolve('effect/Layer')).href);
const FileSystem=await import(pathToFileURL(require.resolve('effect/FileSystem')).href);
const Path=await import(pathToFileURL(require.resolve('effect/Path')).href);
const Stream=await import(pathToFileURL(require.resolve('effect/Stream')).href);
const Sink=await import(pathToFileURL(require.resolve('effect/Sink')).href);
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const Digest=await import(pathToFileURL(require.resolve('effect/crypto/Digest')).href);
const PlatformError=await import(pathToFileURL(require.resolve('effect/PlatformError')).href);
const Child=await import(pathToFileURL(require.resolve('effect/process/ChildProcessSpawner')).href);
const TestClock=await import(pathToFileURL(require.resolve('effect/testing/TestClock')).href);
const {HttpClient,HttpClientResponse}=await import(pathToFileURL(require.resolve('effect/http')).href);
const Host=await import(pathToFileURL(module('packages/shared/src/HostProcess.ts')).href);
const Net=await import(pathToFileURL(module('packages/shared/src/Net.ts')).href);
const Config=await import(pathToFileURL(module('apps/server/src/config.ts')).href);
const contractsModule=await import(pathToFileURL(contracts+'/src/index.ts').href);
const projectError=error=>({_tag:error._tag,hostId:error.hostId,step:error.step,message:error.message});
async function scenario(input){
 const commands=[],modes=[],owners=[],phases=[],statuses=[];let attempts=0;let nextPort=45000;let active=0;
 const tools={hub:{requiredVersion:'0.19.1',installedVersions:['0.19.1'],runningVersion:null},agent:{requiredVersion:'0.21.31',installedVersions:['0.21.31'],runningVersion:null}};
 const remote={nodePath:'/fixture/node',platforms:[{platform:'ios',available:true},{platform:'android',available:false,reason:'fixture absent'}],tools};
 const spawner=Child.make(command=>Effect.gen(function*(){
  if(command._tag!=='StandardCommand')throw Error('Unexpected native command');
  const forwarding=command.args.includes('-L');let mode='';let output='';let commandExit=0;let commandStderr='';
  if(!forwarding){
   const stdin=command.options.stdin;if(!stdin||typeof stdin!=='object'||!Stream.isStream(stdin.stream))throw Error('Missing script');
   const text=yield* stdin.stream.pipe(Stream.decodeText(),Stream.runFold(()=>'',(a,b)=>a+b));
   mode=/const mode = "([^"]+)"/.exec(text)?.[1]??'';
   if(mode){modes.push(mode);owners.push(/const owner = "([^"]+)"/.exec(text)?.[1]??'');}
   output=mode==='probe'?JSON.stringify(remote):mode==='start'||mode==='agent-start'?JSON.stringify({...remote,hubPort:1234,helpers:{serveSimAxSettings:null,serveSimCli:null},...(mode==='agent-start'?{daemonPort:1235,token:'private-fixture',entryPath:'/fixture/agent.mjs'}:{})}):'fixture-output';
  }
  if(!forwarding&&mode===''&&input.runFailure){output=input.runFailure.stdout;commandStderr=input.runFailure.stderr;commandExit=input.runFailure.code;}
  commands.push({command:command.command,args:command.args,forwarding,stdin:forwarding?command.options.stdin:'source-script-not-executed',mode});
  if(forwarding){if(attempts++<input.failForwards)return yield* PlatformError.systemError({_tag:'AlreadyExists',module:'ChildProcess',method:'spawn',description:'Port already bound'});active++;yield* Effect.addFinalizer(()=>Effect.sync(()=>{active--;}));}
  return Child.makeHandle({pid:Child.ProcessId(123),stdout:Stream.make(new TextEncoder().encode(output)),stderr:Stream.make(new TextEncoder().encode(commandStderr)),all:Stream.empty,exitCode:forwarding?Effect.never:Effect.succeed(Child.ExitCode(commandExit)),isRunning:Effect.succeed(forwarding),kill:()=>Effect.void,stdin:Sink.drain,getInputFd:()=>Sink.drain,getOutputFd:()=>Stream.empty,unref:Effect.succeed(Effect.void)});
 }));
 const filesystem=FileSystem.makeNoop({readFileString:file=>input.environmentId===null?Effect.fail(new Error('private absent')):Effect.succeed(input.environmentId)});
 const http=HttpClient.make(request=>Effect.succeed(HttpClientResponse.fromWeb(request,new Response('ok'))));
 const program=Effect.gen(function*(){
  const host=yield* native.make(input.config,()=>Effect.void,(status,detail)=>Effect.sync(()=>{statuses.push({status,...(detail===undefined?{}:{detail})});}));
  const result=yield* host.ensureReady((phase,detail)=>Effect.sync(()=>{phases.push({phase,...(detail===undefined?{}:{detail})});})).pipe(Effect.result);
  let error=result._tag==='Failure'?projectError(result.failure):undefined;
  if(!error&&input.agent){const result=yield* host.ensureAgentReady((phase,detail)=>Effect.sync(()=>{phases.push({phase,...(detail===undefined?{}:{detail})});})).pipe(Effect.result);if(result._tag==='Failure')error=projectError(result.failure);}
  let runResult;
  if(!error&&input.run){const ready=yield* host.current;runResult=yield* ready.run(input.run.command,input.run.args);}
  const summary=yield* host.summary;const current=(yield* host.current)!==null;const activeBeforeStop=active;
  yield* host.stop;
  return {commands,modes,owners,phases,statuses,summary,current,activeBeforeStop,activeAfterStop:active,...(runResult?{runResult}:{}),...(error?{error}:{})};
 }).pipe(Effect.scoped,Effect.provideService(Child.ChildProcessSpawner,spawner),Effect.provideService(FileSystem.FileSystem,filesystem),Effect.provideService(Config.ServerConfig,{stateDir:input.stateDir,environmentIdPath:'/fixture/environment-id'}),Effect.provideService(Net.NetService,{reserveLoopbackPort:()=>Effect.sync(()=>nextPort++)}),Effect.provideService(HttpClient.HttpClient,http),Effect.provideService(Host.Platform,input.platform),Effect.provideService(Host.Environment,{HOME:'/fixture/home',PATH:'/fixture/bin'}),Effect.provide(Path.layer),Effect.provide(Digest.layerWebCrypto),Effect.provide(TestClock.layer()));
 return await Effect.runPromise(program);
}
const cases=[];
for(const platform of ['linux','darwin','win32'])for(const identityFile of [undefined,"/fixture/keys/a'b key"])for(const port of [undefined,22,2222])for(const environmentId of [null,'fixture-environment\n'])for(const failForwards of [0,1,2,3])for(const agent of [false,true]){
 const input={platform,environmentId,stateDir:'/fixture/profile/state',config:{kind:'ssh',id:'mini',label:'Private Mac',target:'user@private.example',...(identityFile?{identityFile}:{}),...(port?{port}:{})},failForwards,agent,run:{command:"/fixture/node path",args:["-e","console.log('private')","arg $() `quoted`\n"]}};
 cases.push({input,expected:await scenario(input)});
}
for(const runFailure of [
 {code:1,stdout:'',stderr:''},{code:7,stdout:' useful message \n',stderr:''},{code:2,stdout:'ignored',stderr:' stderr message \n'},
 {code:1,stdout:'{"token":"private-fixture","access_token":"private-fixture","credential":"private-fixture"}',stderr:''},
 {code:1,stdout:'{"bearerToken":"private-fixture","pairingToken":"private-fixture"}',stderr:''},
 {code:1,stdout:'a'.repeat(3999),stderr:''},{code:1,stdout:'b'.repeat(4000),stderr:''},{code:1,stdout:'c'.repeat(4001),stderr:''},
 {code:1,stdout:'prefix '.repeat(800)+'\nPANIC: private SDK fixture',stderr:''},
]){
 const input={platform:'linux',environmentId:'private-environment',stateDir:'/fixture/state',config:{kind:'ssh',id:'mini',label:'Private host',target:'user@private.example'},failForwards:0,agent:false,run:{command:'emulator',args:['-list-avds']},runFailure};cases.push({input,expected:await scenario(input)});
}
const configCases=[];
const base={id:'mini',label:' Private Mac ',target:' user@host '};
for(const key of ['kind','id','label','target','identityFile','port'])for(const value of [null,false,0,1,22,65535,65536,-1,1.5,'',' ','ssh','local','mini','-oProxyCommand=bad','u@host','host name','host\u0085name','host\u2028name',"/fixture/key'a",' mini ', 'x'.repeat(128),'x'.repeat(129)]){
 const input={...base,[key]:value};let expected;try{expected={value:await Effect.runPromise(Schema.decodeUnknownEffect(contractsModule.SshDeviceHostConfig)(input))};}catch{expected={invalid:true};}configCases.push({input,expected});
}
const quoteCases=['',"a'b",'simple','$() `exec`','line\nnext','\uFEFF space ','λ雪'].map(input=>({input,expected:script.quoteRemoteArg(input)}));
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,remoteEnvironment:script.remoteDeviceEnvironment,cases,configCases,quoteCases},null,2)+'\n');console.log(cases.length+' native complete SSH lifecycle cases, '+configCases.length+' config codecs');
