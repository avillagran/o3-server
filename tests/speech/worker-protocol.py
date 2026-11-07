"""Physical pinned-factory transport checks with an explicitly synthetic engine."""
import base64,hashlib,json,os,pathlib,selectors,shutil,signal,struct,subprocess,tempfile,time
assert pathlib.Path('/run/.containerenv').is_file(), 'Private rootless Podman required'
ROOT=pathlib.Path(tempfile.mkdtemp(prefix='speech-protocol-',dir='/tmp'));ROOT.chmod(0o700)
NODE='/node-bin/node'
MAX=32*1024*1024
class Peer:
 def __init__(self,mode,acceleration='cpu'):
  self.root=ROOT/mode;self.root.mkdir(mode=0o700);self.home=self.root/'home';self.home.mkdir(mode=0o700)
  for name in ['worker.mjs','native-worker.mjs','native-manifest.json','LICENSE']:
   shutil.copyfile('/server/runtime/speech/'+name,self.root/name);(self.root/name).chmod(0o400)
  shutil.copyfile('/server/runtime/speech/text/manifest.json',self.root/'text-manifest.json')
  for name in ['custom-words.mjs','filler-words.mjs','native-language.mjs','model.mjs']:
   shutil.copyfile('/server/runtime/speech/text/'+name,self.root/('text-'+name));(self.root/('text-'+name)).chmod(0o400)
  self.engine=self.root/'synthetic-engine.mjs';shutil.copyfile('/server/tests/speech/synthetic-engine.mjs',self.engine);self.engine.chmod(0o400)
  self.model=self.root/'synthetic-model.gguf';self.model.write_text(mode);self.model.chmod(0o400)
  seal=lambda file:{'path':str(file),'sha256':hashlib.sha256(file.read_bytes()).hexdigest(),'bytes':str(file.stat().st_size)}
  self.owner='synthetic-owner-'+mode+'-00000001'
  spec={'version':1,'owner':self.owner,'nativeSha256':hashlib.sha256((self.root/'native-worker.mjs').read_bytes()).hexdigest(),'textManifestSha256':hashlib.sha256((self.root/'text-manifest.json').read_bytes()).hexdigest(),'engine':seal(self.engine),'model':seal(self.model),'acceleration':acceleration}
  data=json.dumps(spec).encode();(self.root/'spec.json').write_bytes(data);(self.root/'spec.json').chmod(0o400)
  self.error=(self.root/'stderr.log').open('wb')
  self.process=subprocess.Popen([NODE,str(self.root/'worker.mjs'),str(self.root),hashlib.sha256(data).hexdigest()],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.error,start_new_session=True,env={'HOME':str(self.home),'TMPDIR':str(self.home),'PATH':'','ELECTRON_RUN_AS_NODE':'1'})
  self.buffer=bytearray();self.selector=selectors.DefaultSelector();self.selector.register(self.process.stdout,selectors.EVENT_READ)
  hello=self.read();assert hello['type']=='hello' and hello['owner']==self.owner and hello['node']=='26.13.0'
 def read(self):
  end=time.monotonic()+20
  while b'\n' not in self.buffer:
   remaining=end-time.monotonic()
   if remaining<=0 or not self.selector.select(remaining):raise RuntimeError('Bounded native bridge response deadline')
   block=os.read(self.process.stdout.fileno(),64*1024)
   if not block:raise RuntimeError('Native bridge EOF')
   self.buffer.extend(block)
   if len(self.buffer)>MAX:raise RuntimeError('Native bridge response exceeded32MiB')
  line,_,rest=self.buffer.partition(b'\n');self.buffer=bytearray(rest);return json.loads(line)
 def write(self,id,kind,**args):
  data=json.dumps(dict(owner=self.owner,id=id,kind=kind,**args)).encode()+b'\n';assert len(data)<=MAX
  self.process.stdin.write(data);self.process.stdin.flush()
 def call(self,id,kind,**args):
  self.write(id,kind,**args);answer=self.read();assert answer['owner']==self.owner and answer['id']==id;return answer
 def success(self,id,kind,**args):
  answer=self.call(id,kind,**args);assert answer['ok'],answer;return answer['result']
 def events(self):return [json.loads(line)for line in (self.home/'events.jsonl').read_text().splitlines()]
 def stop(self):
  self.process.send_signal(signal.SIGTERM)
  try:code=self.process.wait(timeout=5)
  except subprocess.TimeoutExpired:
   os.killpg(self.process.pid,signal.SIGKILL);self.process.wait(timeout=5);raise
  self.selector.close();self.error.close();assert code==0,code
  for pid in {event['pid']for event in self.events()}:
   try:os.kill(pid,0)
   except ProcessLookupError:continue
   raise AssertionError('Actual native child remains after graceful disposal')

def encoded(length,first=0.25,last=-0.75):
 raw=bytearray(length);raw[:4]=struct.pack('<f',first);raw[-4:]=struct.pack('<f',last if length>4 else first);return base64.b64encode(raw).decode()
options={'timestamps':'none','language':'en'}
results=[]
p=Peer('buffered')
try:
 meta=p.success(1,'load');assert meta['languages']==['en-US','de-DE'] and meta['supportsLongForm']
 for id,length in [(2,4),(3,19_200_000)]:
  answer=json.loads(p.success(id,'transcribe',pcm=encoded(length),options=options,filterSilence=False)['text']);assert answer['bytes']==length and answer['length']==length//4 and answer['last']==(-0.75 if length>4 else 0.25),answer
  results.append({'case':'buffered','bytes':length,'actual':answer})
 allowed={'HOME','TMPDIR','PATH','ELECTRON_RUN_AS_NODE','TRANSCRIBE_BACKENDS','NODE_CHANNEL_FD','NODE_CHANNEL_SERIALIZATION_MODE'}
 assert set(p.events()[0]['environment'])<=allowed,p.events()[0]
finally:p.stop()
p=Peer('stream')
try:
 p.success(1,'load')
 for offset,language,value in [(2,'en',0.25),(5,'de',0.5)]:
  p.success(offset,'begin',options=dict(options,language=language),filterSilence=False)
  data=base64.b64encode(struct.pack('<f',value)*512).decode()
  feed=p.success(offset+1,'feed',pcm=data);assert feed['revision']==1 and feed['text']=={'committed':language+':'+str(value),'tentative':''},feed
  assert p.success(offset+2,'finish')==language+':'+str(value)
 results.append({'case':'turn-separation','feeds':[e['language']for e in p.events()if e['kind']=='feed']})
 assert results[-1]['feeds']==['en','de']
finally:p.stop()
p=Peer('fallback','auto')
try:
 p.success(1,'load');assert [e['backend']for e in p.events()if e['kind']=='load']==['auto','cpu']
 results.append({'case':'native-cpu-fallback','attempts':2})
finally:p.stop()
for mode in ['exit','disconnect']:
 p=Peer(mode)
 try:
  p.success(1,'load');failed=p.call(2,'transcribe',pcm=encoded(4),options=options,filterSilence=False);assert not failed['ok']
  again=p.call(3,'transcribe',pcm=encoded(4),options=options,filterSilence=False);assert not again['ok'] and again['error']=='Speech process stopped.'
  assert len([e for e in p.events()if e['kind']=='load'])==1
  results.append({'case':mode,'error':failed['error'],'noReplay':True})
 finally:p.stop()
p=Peer('hang')
p.success(1,'load');p.write(2,'transcribe',pcm=encoded(4),options=options,filterSilence=False)
end=time.monotonic()+5
while not (p.home/'entered').exists():
 if time.monotonic()>=end:raise RuntimeError('Synthetic native inference entry was not observed')
 time.sleep(.005)
p.stop();results.append({'case':'hung-native-abort','actualChildReaped':True})
pathlib.Path('/out/protocol-results.json').write_text(json.dumps({'node':'26.13.0','engine':'EXPLICIT SYNTHETIC MODULE, NOT transcribe-cpp inference','results':results},indent=2)+'\n')
print(json.dumps(results))
