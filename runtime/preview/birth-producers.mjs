// Canonical Page event bindings installed before the birth owner releases author
// navigation. These retain actual objects, not a formatted Status projection.
// Installation is not evidence of prior absence or completed download/adoption.
const EVENTS=Object.freeze(['request','load','framenavigated','requestfailed','dialog','filechooser','download','popup','close','crash']);
export async function installBirthProducers({page,cdp,targetId}){
 if(typeof targetId!=='string'||!targetId)throw Error('Owned birth target required');
 const records=[];let sequence=0,unknown=false,retired=false;
 const handlers=new Map();
 const record=(event,value)=>{
  if(retired)return;
  if(sequence>=Number.MAX_SAFE_INTEGER||records.length>=1024){unknown=true;return;}
  records.push(Object.freeze({sequence:++sequence,event,value}));
 };
 const bind=(event,handler)=>{page.on(event,handler);handlers.set(event,handler);};
 try{
  bind('request',request=>{if(request.isNavigationRequest()&&request.frame()===page.mainFrame())record('request',request);});
  bind('load',()=>record('load',page.mainFrame()));
  bind('framenavigated',frame=>{if(frame===page.mainFrame())record('framenavigated',frame);});
  bind('requestfailed',request=>{if(request.isNavigationRequest()&&request.frame()===page.mainFrame())record('requestfailed',request);});
  bind('dialog',dialog=>record('dialog',dialog));
  bind('filechooser',chooser=>record('filechooser',chooser));
  bind('download',download=>record('download',download));
  bind('popup',popup=>record('popup',popup));
  bind('close',()=>record('close',page));
  bind('crash',()=>record('crash',page));
  // Original local pages always intercept uploads. Await the genuine fixed
  // session ACK, rather than treating Playwright's asynchronous listener hook
  // as completed interception. Desktop/adopted pages never enter this installer.
  await cdp.send('Page.setInterceptFileChooserDialog',{enabled:true});
 }catch(error){for(const[event,handler]of handlers)page.off(event,handler);throw error;}
 const receipt=Object.freeze({kind:'canonical-page-event-bindings',targetId,events:EVENTS});
 return Object.freeze({
  receipt,
  // This private ledger retains actual event objects. Consumers must implement
  // original chooser/save/adoption semantics and cannot turn missing history,
  // overflow or unobserved initial fields into empty Status values.
  observations(){return Object.freeze({initial:'unobserved',overflow:unknown,records:Object.freeze([...records])});},
  close(){if(retired)return;retired=true;for(const[event,handler]of handlers)page.off(event,handler);handlers.clear();}
 });
}
