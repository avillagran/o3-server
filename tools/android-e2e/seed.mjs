import fs from 'node:fs';
const args=process.argv.slice(2);
const [tok,cwd]=args[0]==='--session-file' ? [fs.readFileSync(args[1],'utf8').trim(),args[2]] : args;
const base='127.0.0.1:38900';
const ws=new WebSocket(`ws://${base}/ws?orchestrationProtocol=2`,{headers:{Authorization:`Bearer ${tok}`}});
let n=1;const pend={};const rpc=(tag,payload)=>new Promise((r,j)=>{const id=String(n++);pend[id]=[r,j];ws.send(JSON.stringify({_tag:'Request',id,tag,payload,headers:[]}))});
const seen=[];let threadId='th-'+Date.now();let done;const fin=new Promise(r=>done=r);
ws.onmessage=e=>{const m=JSON.parse(e.data);if(m._tag==='Ping')ws.send('{"_tag":"Pong"}');
 if(m._tag==='Exit'&&pend[m.requestId]){pend[m.requestId][m.exit._tag==='Success'?0:1](m.exit.value??m.exit.cause)}
 if(m._tag==='Chunk'){ws.send(JSON.stringify({_tag:'Ack',requestId:m.requestId}));for(const v of m.values){const t=v.event?.type||v.kind;seen.push(t);
  if(v.event?.type==='message.updated'&&v.event.payload?.role==='assistant')console.log('A:',v.event.payload.text?.slice(0,120),v.event.payload.streaming);
  if(v.event?.type==='run.updated'||v.event?.type==='run.created')console.log('run',v.event.payload?.status);
  if(/run\./.test(t||'')&&['completed','failed','interrupted'].includes(v.event.payload?.status))done();}}};
ws.onopen=async()=>{try{
 const cfg=await rpc('server.getConfig',{});const pv=cfg.providers.find(p=>p.instanceId==='claudeAgent');console.log(pv.status,pv.auth?.status,pv.models?.map(m=>m.slug).join(','));const model=pv.models.find(m=>/haiku|mini|spark/.test(m.slug||m.id))||pv.models[0];const ms=model.slug||model.id;console.log('model',ms);
 const at=new Date().toISOString();const pr=await fetch(`http://${base}/api/projects/mutate`,{method:'POST',headers:{Authorization:`Bearer ${tok}`,'content-type':'application/json'},body:JSON.stringify({type:'project.create',commandId:'pclaudeAgent',projectId:'projclaudeAgent',title:'t',workspaceRoot:cwd})});console.log('proj',pr.status,(await pr.text()).slice(0,160));
 await rpc('orchestration.dispatchCommand',{type:'thread.create',commandId:'c1',threadId,projectId:'projclaudeAgent',title:'t',modelSelection:{instanceId:'claudeAgent',model:ms},runtimeMode:'full-access',interactionMode:'default',branch:null,worktreePath:null,createdAt:at});
 console.log('seeded',threadId);
}catch(e){console.log('ERR',JSON.stringify(e))}process.exit(0)};
