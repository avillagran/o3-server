#!/usr/bin/env python3
"""Actual CLI/CPU HTTP and media acceptance; only private synthetic silence."""
import os,pathlib,json,subprocess,time,signal,socket,struct,hashlib,base64
from http.client import HTTPConnection
os.umask(0o077)
assert subprocess.check_output(['/node-bin/node','--version'],env={'HOME':'/tmp/home','PATH':''},text=True).strip()=='v26.13.0'
assert pathlib.Path('/run/.containerenv').is_file(), 'Private Podman runtime required'
root=pathlib.Path('/evidence');profile=root/'profile';profile.mkdir(mode=0o700);userdata=profile/'userdata';userdata.mkdir(mode=0o700)
settings={'speechModelId':'handy-computer/moonshine-tiny-gguf','speechAcceleration':'cpu','speechModelUnloadTimeout':'never'}
(userdata/'settings.json').write_text(json.dumps(settings))
models=userdata/'speech'/'models';models.mkdir(mode=0o700,parents=True)
with open('/inputs/model.gguf','rb') as source,(models/'moonshine-tiny-Q8_0.gguf').open('xb') as dest:
 while block:=source.read(1024*1024):dest.write(block)
assert hashlib.sha256((models/'moonshine-tiny-Q8_0.gguf').read_bytes()).hexdigest()=='2fd348d7b38f97d309cc3ec6848f3f57f537b80244950f07d2637e463f95a3a1'
cli=['/runtime/o3-server'];common=['--base-dir',str(profile)]
token=subprocess.check_output(cli+['auth','session','issue',*common,'--ttl','1h','--subject','private-speech-acceptance','--token-only'],text=True).strip()
options={'speechLanguage':'en','speechTranslateToEnglish':False,'speechRemoveFillerWords':False,'speechCustomWords':[],'speechSilenceFiltering':False}
result={'scope':'actual CLI + sealed 140-file engine + native tiny CPU, private synthetic silence only','capabilityEnabled':'explicit verified runtime only','checks':[]}
port=43191
children=[]

def http(method,path,body=None,content='application/json'):
 connection=HTTPConnection('127.0.0.1',port,timeout=30)
 headers={'Authorization':'Bearer '+token,'Content-Type':content}
 connection.request(method,path,body=body,headers=headers);response=connection.getresponse();data=response.read();status=response.status;connection.close()
 return status,json.loads(data) if data else None

def start(extra):
 log=(root/('serve-'+str(len(list(root.glob('serve-*'))))+'.log')).open('w')
 process=subprocess.Popen(cli+['serve',*common,'--host','127.0.0.1','--port',str(port),'--no-bridge',*extra],stdout=log,stderr=subprocess.STDOUT)
 children.append(process)
 deadline=time.monotonic()+30
 while time.monotonic()<deadline:
  if process.poll() is not None:raise AssertionError('CLI exited during startup')
  try:
   response=http('GET','/api/voice/status')
   return process,response
  except (ConnectionError,OSError):time.sleep(.05)
 raise AssertionError('CLI startup did not listen')

def stop(process):
 process.send_signal(signal.SIGTERM);assert process.wait(timeout=15)==0

def exact(sock,n):
 out=b''
 while len(out)<n:
  part=sock.recv(n-len(out));assert part;out+=part
 return out

def send(sock,opcode,data):
 mask=os.urandom(4);n=len(data);header=bytes([0x80|opcode,0x80|(n if n<126 else 126 if n<65536 else 127)])
 if n>=126:header+=struct.pack('!H' if n<65536 else '!Q',n)
 sock.sendall(header+mask+bytes(value^mask[i%4] for i,value in enumerate(data)))

def recv(sock):
 while True:
  a,b=exact(sock,2);n=b&127
  if n==126:n=struct.unpack('!H',exact(sock,2))[0]
  if n==127:n=struct.unpack('!Q',exact(sock,8))[0]
  assert n<=256*1024
  mask=exact(sock,4) if b&128 else None;data=exact(sock,n)
  if mask:data=bytes(value^mask[i%4] for i,value in enumerate(data))
  if a&15==9:send(sock,10,data);continue
  assert a&15==1,('Unexpected frame',a&15)
  return json.loads(data)

process=None
try:
 process,status=start(['--speech-node','/node-bin/node','--speech-bundle','/inputs'])
 assert status[0]==200,status
 result['checks'].append('configured status authenticated')
 descriptor=http('GET','/.well-known/t3/environment')[1]['capabilities'];assert descriptor['voiceTranscription'] is True and not descriptor.get('cloudVoiceTranscription',False)
 result['checks'].append('runtime-gated local capability only')
 assert http('GET','/api/voice/models?benchmarks=1')[0]==200
 assert http('POST','/api/voice/models/prepare',b'{}')[0]==204
 pcm=b'\0'*(16000*4);encoded=json.dumps(options,separators=(',',':')).encode();body=struct.pack('<I',len(encoded))+encoded+pcm
 status,response=http('POST','/api/voice/transcribe',body,'application/octet-stream');assert status==200 and isinstance(response.get('text'),str),(status,response)
 result['bufferedTranscriptUtf8Bytes']=len(response['text'].encode());result['checks'].append('actual CPU buffered HTTP')
 sock=socket.create_connection(('127.0.0.1',port),timeout=30);key=base64.b64encode(os.urandom(16)).decode()
 sock.sendall(('GET /ws/voice HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: '+key+'\r\nAuthorization: Bearer '+token+'\r\n\r\n').encode())
 header=b''
 while not header.endswith(b'\r\n\r\n'):header+=exact(sock,1);assert len(header)<16384
 assert header.startswith(b'HTTP/1.1 101'),header
 send(sock,1,json.dumps({'type':'start','options':options}).encode());assert recv(sock)=={'type':'ready'}
 for offset in range(0,len(pcm),64000):send(sock,2,pcm[offset:offset+64000])
 send(sock,1,b'{"type":"finish"}')
 events=[]
 while True:
  event=recv(sock);events.append(event['type'])
  if event['type']=='finished':result['streamTranscriptUtf8Bytes']=len(event['text'].encode());break
  assert event['type']=='update',event
 send(sock,8,b'\x03\xe8');sock.close();result['streamEvents']=events;result['checks'].append('actual CPU media WebSocket finish')
 assert http('POST','/api/voice/acceleration',b'{"acceleration":"cpu"}')[0]==200
 assert http('POST','/api/voice/model-unload-timeout',b'{"timeout":"never"}')[0]==200
 cache=next((userdata/'speech').glob('node-26.13.0-*'));identity=(cache.stat().st_dev,cache.stat().st_ino,cache.stat().st_size)
 stop(process);process=None;result['checks'].append('actual CLI owned shutdown')
 process,status=start(['--speech-node','/node-bin/node','--speech-bundle','/inputs']);assert status[0]==200
 assert identity==(cache.stat().st_dev,cache.stat().st_ino,cache.stat().st_size)
 stop(process);process=None;result['checks'].append('verified immutable Node cache reused on restart')
 process,status=start(['--speech-node','/node-bin/node','--speech-bundle','/missing-bundle']);assert status[0]==404,status
 assert http('GET','/.well-known/t3/environment')[1]['capabilities']['voiceTranscription'] is False
 stop(process);process=None;result['checks'].append('invalid graph leaves Core live and speech unavailable')
 process,status=start([]);assert status[0]==404
 assert http('GET','/.well-known/t3/environment')[1]['capabilities']['voiceTranscription'] is False
 stop(process);process=None;result['checks'].append('default unconfigured startup has no local capability')
 result['returncode']=0
finally:
 for owned in children:
  if owned.poll() is None:owned.send_signal(signal.SIGTERM);owned.wait(timeout=15)
 (root/'result.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
