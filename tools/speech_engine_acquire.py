"""Private, script-free materialization of source-pinned speech packages."""
import urllib.request,urllib.parse,json,pathlib,hashlib,base64,tarfile,os,stat
assert pathlib.Path('/run/.containerenv').exists(),'Acquisition requires private rootless Podman'
def present(path):
 try:path.lstat();return True
 except FileNotFoundError:return False

def safe_open(path):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|os.O_NONBLOCK)
 if not stat.S_ISREG(os.fstat(fd).st_mode):os.close(fd);raise ValueError('Cache input is not a regular file')
 return os.fdopen(fd,'rb')

def read_cache(path,maximum):
 with safe_open(path) as source:data=source.read(maximum+1)
 if len(data)>maximum:raise ValueError('Cache input exceeds its bound')
 return data

out=pathlib.Path('/out');source_root=pathlib.Path('/source');cache=pathlib.Path('/archive-cache');receipts=[]
manifest=json.loads((source_root/'runtime/speech/engine-packages-4314.json').read_text())
catalog_bytes=(source_root/'runtime/speech/catalog-4314.json').read_bytes()
assert hashlib.sha256(catalog_bytes).hexdigest()==manifest['catalogSha256']

graph=out/'graph';graph.mkdir(mode=0o700)
for index,pinned in enumerate(manifest['packages']):
 name=pinned['name'];version=pinned['version'];sri=pinned['integrity']
 metadata_cache=cache/f'{index}.json'
 if present(metadata_cache):data=read_cache(metadata_cache,2*1024*1024)
 else:
  url='https://registry.npmjs.org/'+urllib.parse.quote(name,safe='')+'/'+version
  with urllib.request.urlopen(url,timeout=30) as response:data=response.read(2*1024*1024+1)
 assert len(data)<=2*1024*1024;meta=json.loads(data)
 assert meta['name']==name and meta['version']==version and meta['dist']['integrity']==sri
 assert meta['dist']['tarball']==pinned['tarball'] and meta['dist']['unpackedSize']==pinned['unpackedSize'] and meta['dist']['fileCount']==pinned['fileCount']
 (out/f'{index}.json').write_bytes(data)
 archive=out/f'package-{index}.tgz';digest=hashlib.sha512();size=0
 with safe_open(cache/f'package-{index}.tgz') if present(cache/f'package-{index}.tgz') else urllib.request.urlopen(meta['dist']['tarball'],timeout=60) as response, archive.open('xb') as target:
  while chunk:=response.read(64*1024):
   size+=len(chunk);assert size<=256*1024*1024;digest.update(chunk);target.write(chunk)
 assert 'sha512-'+base64.b64encode(digest.digest()).decode()==sri
 destination=graph/'node_modules'/name;destination.mkdir(mode=0o700,parents=True)
 files=[];seen=set();total=0
 with tarfile.open(archive,'r:gz') as tar:
  for member in tar:
   parts=member.name.split('/');assert parts[0]=='package' and all(p not in ('..','') for p in parts[1:]),member.name
   assert not any(p=='.' for p in parts[1:]);assert len(member.name)<4096
   if member.isdir():continue
   assert member.isfile() and not member.issym() and not member.islnk(),member.name
   relative=pathlib.PurePosixPath(*parts[1:]);assert str(relative) not in seen;seen.add(str(relative));total+=member.size;assert total<=meta['dist']['unpackedSize']
   path=destination.joinpath(*relative.parts);path.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
   fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o500 if member.mode&0o111 else 0o400)
   sha=hashlib.sha256();written=0
   with os.fdopen(fd,'wb') as target,tar.extractfile(member) as source:
    while chunk:=source.read(64*1024):sha.update(chunk);written+=len(chunk);target.write(chunk)
   assert written==member.size
   files.append({'path':str(path.relative_to(graph)),'bytes':written,'sha256':sha.hexdigest(),'mode':oct(path.stat().st_mode&0o777)})
 assert total==meta['dist']['unpackedSize'] and len(files)==meta['dist']['fileCount']
 package=json.loads((destination/'package.json').read_text());assert package['name']==name and package['version']==version
 receipts.append({'name':name,'version':version,'sri':sri,'archiveBytes':size,'unpackedBytes':total,'license':meta.get('license'),'source':meta['dist']['tarball'],'files':files,'installScriptsExecuted':False})
(out/'engine-receipt.json').write_text(json.dumps({'nativeRevision':manifest['nativeRevision'],'platform':manifest['platform'],'packages':receipts,'manifestSha256':hashlib.sha256((source_root/'runtime/speech/engine-packages-4314.json').read_bytes()).hexdigest()},indent=2))
model_id=os.environ.get('O3_SPEECH_ACQUIRE_MODEL')
if not model_id:
 print(json.dumps({'packages':len(receipts),'engineBytes':sum(r['unpackedBytes'] for r in receipts),'model':None}));raise SystemExit(0)
catalog=json.loads(catalog_bytes);matches=[model for model in catalog['models'] if model['id']==model_id]
assert len(matches)==1,'Select an existing pinned native catalog model'
model=matches[0];revision=model['revision'];base='https://huggingface.co/'+model['id']
if present(cache/'model-receipt.json'):
 cached_model=json.loads(read_cache(cache/'model-receipt.json',2*1024*1024).decode('utf-8'));assert cached_model['catalog']['id']==model_id
 for file in ['model-metadata.json','model-README.md','model-LICENSE','model-LICENSE.md','model-LICENSE.txt']:
  if present(cache/file):(out/file).write_bytes(read_cache(cache/file,2*1024*1024))
else:
 with urllib.request.urlopen(base.replace('https://huggingface.co/','https://huggingface.co/api/models/')+'/revision/'+revision,timeout=30) as response:data=response.read(2*1024*1024+1)
 assert len(data)<=2*1024*1024;(out/'model-metadata.json').write_bytes(data)
 metadata=json.loads(data);assert metadata['sha']==revision
 for sibling in metadata.get('siblings',[]):
  name=sibling['rfilename']
  if name.lower() in ['readme.md','license','license.txt','license.md','notice']:
   with urllib.request.urlopen(base+'/resolve/'+revision+'/'+urllib.parse.quote(name),timeout=30) as response:data=response.read(1024*1024+1)
   assert len(data)<=1024*1024;(out/('model-'+name.replace('/','-'))).write_bytes(data)
url=base+'/resolve/'+revision+'/'+model['filename'];digest=hashlib.sha256();count=0
with safe_open(cache/'model.gguf') if present(cache/'model.gguf') else urllib.request.urlopen(url,timeout=60) as response,(out/'model.gguf').open('xb') as target:
 while chunk:=response.read(64*1024):
  count+=len(chunk);assert count<=model['size'];digest.update(chunk);target.write(chunk)
assert count==model['size'] and digest.hexdigest()==model['sha256']
(out/'model-receipt.json').write_text(json.dumps({'catalog':model,'url':url,'bytes':count,'sha256':digest.hexdigest(),'catalogSha256':manifest['catalogSha256']},indent=2))
print(json.dumps({'packages':len(receipts),'engineBytes':sum(r['unpackedBytes'] for r in receipts),'model':model['id'],'modelBytes':count,'sha256':digest.hexdigest()}))
