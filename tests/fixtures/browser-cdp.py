#!/usr/bin/python3
"""Deterministic CDP pipe peer. No browser, network, or user profile access."""
import json,os,sys
profile=next(a.split('=',1)[1] for a in sys.argv if a.startswith('--user-data-dir='))
with open(profile+'/fixture.pid','w') as f:f.write(str(os.getpid()))
with open(profile+'/argv.json','w') as f:json.dump(sys.argv[1:],f)
buf=b''
while True:
    chunk=os.read(3,4096)
    if not chunk:break
    buf+=chunk
    while b'\0' in buf:
        raw,buf=buf.split(b'\0',1)
        v=json.loads(raw)
        with open(profile+'/commands.jsonl','a') as f:f.write(json.dumps(v)+'\n')
        result={}
        if v['method']=='Target.createTarget':result={'targetId':'fixture-target'}
        if v['method']=='Target.attachToTarget':result={'sessionId':'fixture-session'}
        if v['method']=='Page.captureScreenshot':result={'data':'/9j/2Q=='}
        os.write(4,json.dumps({'id':v['id'],'result':result}).encode()+b'\0')
