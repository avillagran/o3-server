#!/usr/bin/env python3
"""Native peer asserting isolated per-instance launch state and model options."""
import json, os, sys
from pathlib import Path
claude = '-p' in sys.argv
assert sys.argv[sys.argv.index('--instance-marker')+1] == 'quoted value'
home = Path(os.environ['CLAUDE_CONFIG_DIR' if claude else 'CODEX_HOME'])
assert os.environ['INSTANCE_MARKER'] == home.name
with (home/'starts').open('a') as f: f.write('start\n')
for line in sys.stdin:
    f = json.loads(line)
    if claude:
        if f['type'] == 'control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id'],'response':{'account':{'email':'fixture@example.invalid','apiProvider':'firstParty','tokenSource':'fixture'},'commands':[{'name':'native-command','description':'Discovered before inference'}]}}}), flush=True)
        elif f['type'] == 'user':
            print(json.dumps({'type':'system','subtype':'init','session_id':home.name}),flush=True)
            print(json.dumps({'type':'result','subtype':'success','session_id':home.name}),flush=True)
    else:
        m,i=f.get('method'),f.get('id')
        if i is None: continue
        if m=='initialize': result={'capabilities':{}}
        elif m=='model/list': result={'data':[{'model':'test','displayName':'Test','isDefault':True,'serviceTiers':[{'id':'fast','name':'Fast'}]}]}
        elif m=='skills/list': result={'data':[{'skills':[{'name':'native-skill','description':'Discovered','path':str(home/'native'),'enabled':True}]}]}
        elif m=='config/read': result={'config':{'service_tier':'fast'}}
        elif m=='account/read': result={'account':{'type':'chatgpt'}}
        elif m=='account/rateLimits/read': result={'rateLimits':{'primary':{'usedPercent':12,'windowDurationMins':300}}}
        elif m in ('thread/start','thread/resume'): result={'thread':{'id':home.name}}
        elif m=='turn/start':
            assert f['params']['serviceTier']=='fast'
            result={'turn':{'id':'turn'}}
        else: result={}
        print(json.dumps({'id':i,'result':result}),flush=True)
        if m=='turn/start': print(json.dumps({'method':'turn/completed','params':{'threadId':home.name,'turn':{'id':'turn','status':'completed'}}}),flush=True)
