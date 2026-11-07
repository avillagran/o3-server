#!/usr/bin/env node
// Execute pinned DeviceActions with a recording runner, never a host/device command.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout || !contracts || !output) throw Error('usage: generate-device-action-fixtures.mjs T3_CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const path='apps/server/src/device/DeviceActions.ts';
const source=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2') throw Error('Expected pinned Effect 4.1.2');
const rewrite=stripTypeScriptTypes(source).replace(/from\s+(["'])([^"']+)\1/g,(all,quote,specifier)=>{
  if(specifier==='@t3tools/contracts') return 'from '+JSON.stringify(pathToFileURL(contracts+'/src/index.ts').href);
  if(specifier.startsWith('effect/')) return 'from '+JSON.stringify(pathToFileURL(require.resolve(specifier)).href);
  return all;
});
const native=await import('data:text/javascript;base64,'+Buffer.from(rewrite).toString('base64'));
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Result=await import(pathToFileURL(require.resolve('effect/Result')).href);
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const C=await import(pathToFileURL(contracts+'/src/index.ts').href);
const baseline={night:'Night mode: yes\n',font:'1.15\n',animator:'0\n',wifi:'1\n',focus:'mFocusedApp=ActivityRecord{abcd u12 com.example.app/.MainActivity t12}\n'};
const responsesFor=(spec={})=>({...baseline,...spec});
const fixture=(spec={})=>{
  const calls=[];
  const reads=responsesFor(spec.reads);
  const ready={run:(command,args)=>{
    calls.push({command,args});
    const key=args.slice(2).join(' ');
    let stdout='';
    if(key==='shell wm size') stdout=spec.size??'Physical size: 1080x1920\n';
    if(key==='shell cmd uimode night') stdout=reads.night;
    if(key==='shell settings get system font_scale') stdout=reads.font;
    if(key==='shell settings get global animator_duration_scale') stdout=reads.animator;
    if(key==='shell settings get global wifi_on') stdout=reads.wifi;
    if(key==='shell dumpsys window') stdout=reads.focus;
    const permission=args.at(-1);
    const failed=spec.failPermissions?.includes(permission) || (key==='shell wm size' && spec.failSize);
    const unreadable=Object.entries({night:'shell cmd uimode night',font:'shell settings get system font_scale',animator:'shell settings get global animator_duration_scale',wifi:'shell settings get global wifi_on',focus:'shell dumpsys window'}).some(([name,command])=>key===command && reads[name]===null);
    return Effect.succeed({code:failed||unreadable?1:0,stdout:stdout??'',stderr:failed||unreadable?'synthetic command failure':''});
  },helpers:{},nodePath:'/synthetic/node',hub:{origin:'http://127.0.0.1:1'}};
  return {ready,calls};
};
const actionCases=[];
const add=(label,input,spec={})=>actionCases.push({label,input:{deviceId:'emulator-5554',...input},spec});
for(const value of ['light','dark']) add('appearance:'+value,{type:'setAppearance',value});
for(const value of ['small','default','large','extra-large']) add('text:'+value,{type:'setTextSize',value});
for(const setting of ['reduceMotion','networkEnabled','increaseContrast'])for(const value of [false,true])add('toggle:'+setting+':'+value,{type:'setToggle',setting,value});
for(const deviceId of ['emulator-5554','physical-serial'])for(const size of ['Physical size: 1080x1920','Physical size: 2560x1600','Physical size: 1080x1920\nOverride size: 1920x1080'])for(const value of ['portrait','landscape_left','portrait_upside_down','landscape_right'])add('orientation:'+deviceId+':'+size+':'+value,{type:'setOrientation',deviceId,value},{size});
add('orientation:failed-query',{type:'setOrientation',value:'portrait'},{failSize:true});
for(const [latitude,longitude] of [[-90,-180],[90,180],[1e-7,1e-6],[-0,0],[12.3456789,-70.1234567]])add('location:'+latitude+':'+longitude,{type:'setLocation',latitude,longitude});
add('clear-location',{type:'clearLocation'});
for(const permission of ['camera','microphone','photos','contacts','calendar','location','notifications','motion','reminders','media-library','faceid'])for(const decision of ['grant','revoke','reset'])add('permission:'+permission+':'+decision,{type:'setPermission',permission,decision,appId:'com.example.app'});
add('permission:photos-partial',{type:'setPermission',permission:'photos',decision:'grant',appId:'com.example.app'},{failPermissions:['android.permission.READ_MEDIA_IMAGES']});
add('permission:photos-total-failure',{type:'setPermission',permission:'photos',decision:'grant',appId:'com.example.app'},{failPermissions:['android.permission.READ_MEDIA_IMAGES','android.permission.READ_EXTERNAL_STORAGE']});
add('permission:contacts-first-success',{type:'setPermission',permission:'contacts',decision:'revoke',appId:'com.example.app'},{failPermissions:['android.permission.WRITE_CONTACTS']});
for(const url of ["https://example.com/callback?code=1&state=2","https://example.com/it's?a=1&b=2",'https://example.com/a b?key=value','fixture://callback/ñ?data=$(literal);x=1'])add('open-url:'+url,{type:'openUrl',url});
for(const appId of ['com.example.app',"com.example.'quoted'",'com.example.literal;$(payload)','com.example.ñ'])for(const type of ['launchApp','terminateApp'])add(type+':'+appId,{type,appId});
for(const input of [{type:'setLiquidGlass',value:'tinted'},{type:'setColorFilter',value:'grayscale'},{type:'sendPush',appId:'com.example.app',payload:{aps:{alert:'synthetic'}}},{type:'shake'}])add('unsupported:'+input.type,input);
for(const value of actionCases){
  Schema.decodeUnknownSync(C.DeviceActionInput)(value.input);
  const {ready,calls}=fixture(value.spec);
  const result=await Effect.runPromise(Effect.result(native.runDeviceAction(ready,'android',value.input)));
  if(Result.isFailure(result)){
    const error=result.failure;
    value.expected={calls,error:{_tag:error._tag,operation:error.operation,reason:error.reason,message:error.message,...(error.platform?{platform:error.platform}:{}),...(error.exitCode===undefined?{}:{exitCode:error.exitCode})}};
  } else {
    value.expected={calls,ok:true};
  }
}
const detailCases=[];
const read=async(label,reads)=>{
  const {ready}=fixture({reads});
  const expected=await Effect.runPromise(native.readDeviceDetail(ready,'android','emulator-5554'));
  Schema.decodeUnknownSync(C.DeviceSettings)(expected.settings);
  detailCases.push({label,reads:responsesFor(reads),expected});
};
for(const font of [null,'','null','0','0.9','0.900001','1.1','1.249999','1.25','Infinity','NaN','0x1','0b10','0o1','1e-999','\uFEFF0.85\uFEFF'])await read('font:'+font,{font});
for(const animator of [null,'','null','0','-0','0x0','0b0','0o0','1','garbage','Infinity','NaN','1e-999'])await read('animator:'+animator,{animator});
for(const focus of [null,'','mCurrentFocus=null','mCurrentFocus=Window{abc u0 com.example.first/.A}\nmFocusedApp=ActivityRecord{def u2 com.example.second/.B}','mCurrentFocus=bad/naked','mFocusedApp=ActivityRecord{abc u42 com.example.dashed-app/.MainActivity}','mCurrentFocus=Window{abc u0 com.example.ñ/.A}'])await read('focus:'+focus,{focus});
await read('unreadable',{night:null,font:null,animator:null,wifi:null,focus:null});
const data={revision,effect:'4.1.2',source:{path,sha256:crypto.createHash('sha256').update(source).digest('hex')},actionCases,detailCases};
const serialized=Object.entries(data).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');
console.log(`${actionCases.length} action traces and ${detailCases.length} Android detail projections from pinned complete module`);
