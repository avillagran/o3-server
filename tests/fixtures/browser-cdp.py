#!/usr/bin/python3
"""Deterministic CDP pipe peer. No browser, network, or user profile access."""
import json,os,sys
profile=next(a.split('=',1)[1] for a in sys.argv if a.startswith('--user-data-dir='))
with open(profile+'/fixture.pid','w') as f:f.write(str(os.getpid()))
with open(profile+'/argv.json','w') as f:json.dump(sys.argv[1:],f)
sequence=0
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
        if v['method']=='Browser.getVersion':result={'userAgent':'Mozilla/5.0 (X11; Linux x86_64) HeadlessChrome/144.0.0.0'}
        if v['method']=='Target.createTarget':
            sequence+=1
            result={'targetId':'fixture-target-'+str(sequence)}
        if v['method']=='Target.attachToTarget':result={'sessionId':'fixture-session-'+v['params']['targetId']}
        if v['method']=='Page.navigate' and v['params'].get('url','').endswith('/rejected'):result={'errorText':'net::ERR_CONNECTION_REFUSED'}
        if v['method']=='Runtime.evaluate' and 'document.body.innerText' in v['params'].get('expression',''):result={'result':{'value':{'url':'http://localhost:1234/next','title':'Owned fixture','text':'Fixture body'}}}
        if v['method']=='Page.captureScreenshot':result={'data':'/9j/2Q=='}
        os.write(4,json.dumps({'id':v['id'],'result':result}).encode()+b'\0')
