// Token comes from a private file, never argv or retained evidence.
import fs from 'node:fs';
const W=`${process.env.O3_BUILD_ROOT||`${process.env.HOME}/.cache/o3-build`}/android`;
const [tag,payload='{}',output]=process.argv.slice(2);
const ws=new WebSocket('ws://127.0.0.1:38900/ws?orchestrationProtocol=2',{headers:{Authorization:`Bearer ${fs.readFileSync(W+'/session','utf8').trim()}`}});
const timer=setTimeout(()=>{console.error('RPC timeout');process.exit(1)},20000);
ws.onopen=()=>ws.send(JSON.stringify({_tag:'Request',id:'1',tag,payload:JSON.parse(payload),headers:[]}));
ws.onmessage=e=>{const m=JSON.parse(e.data);if(m._tag==='Ping')ws.send('{"_tag":"Pong"}');if(m._tag==='Chunk'){ws.send(JSON.stringify({_tag:'Ack',requestId:m.requestId}));if((tag==='orchestration.subscribeShell'||tag==='terminal.observe')){clearTimeout(timer);const v=JSON.stringify(m.values,null,2);if(output)fs.writeFileSync(output,v);else console.log(v);ws.close();return;}}if(m._tag==='Exit'){clearTimeout(timer);const v=JSON.stringify(m.exit,null,2);if(output)fs.writeFileSync(output,v);else console.log(v);ws.close();if(m.exit._tag!=='Success')process.exitCode=1;}};
