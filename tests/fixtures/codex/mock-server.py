#!/usr/bin/env python3
"""Deterministic peer. Startup results replay captured real frames; turn traffic is synthetic."""
import json, sys
from pathlib import Path
fixture = Path(sys.argv[0]).with_name('real-0.166.0.ndjson')
recorded = [json.loads(line) for line in fixture.read_text().splitlines()]
results = {r['frame']['id']: r['frame'].get('result') for r in recorded if r['direction']=='server' and 'result' in r['frame']}
threads = 0
requests = {}
def send(v):
    print(json.dumps(v), flush=True)
def notify(method, tid, **params):
    send(dict(method=method, params=dict(threadId=tid, turnId='turn-'+tid, **params)))
for line in sys.stdin:
    f=json.loads(line)
    m=f.get('method'); p=f.get('params', {}); i=f.get('id')
    if m=='initialize': result=results[1]
    elif m=='initialized': continue
    elif m=='model/list': result=results[3]
    elif m=='account/read': result=results[2]
    elif m=='thread/start':
        threads+=1;result={'thread':{'id':'thread-'+str(threads)}}
    elif m=='thread/resume':
        assert p['excludeTurns'] is True
        result={'thread':{'id':p['threadId']}}
    elif m=='turn/start' and sum(len(x.get('text','')) for x in p.get('input',[])) > 1048576:
        send(dict(id=i,error={'code':-32602,'message':'Input exceeds the maximum length of 1048576 characters.'}));continue
    elif m=='turn/start' and 'burst' in json.dumps(p.get('input')):
        # A burst of notifications far larger than any per-thread frame count.
        tid=p['threadId']; send(dict(id=i,result={'turn':{'id':'turn-'+tid}}))
        notify('turn/started',tid,turn={'id':'turn-'+tid,'status':'inProgress'})
        for n in range(600):
            notify('item/agentMessage/delta',tid,itemId='message',delta=f'{n},')
        notify('turn/completed',tid,turn={'id':'turn-'+tid,'status':'completed'})
        continue
    elif m=='turn/start':
        tid=p['threadId']; result={'turn':{'id':'turn-'+tid}}
        send(dict(id=i,result=result))
        notify('turn/started',tid,turn={'id':'turn-'+tid,'status':'inProgress'})
        notify('item/agentMessage/delta',tid,itemId='message',delta='hello '+tid)
        request_id=100+int(tid.rsplit('-',1)[-1]); requests[request_id]=tid
        send(dict(id=request_id,method='item/commandExecution/requestApproval',params=dict(threadId=tid,turnId='turn-'+tid,itemId='cmd',command='echo fixture')))
        continue
    elif m=='turn/steer':
        assert p['expectedTurnId']=='turn-'+p['threadId']
        result={'turnId':p['expectedTurnId']}
    elif m=='turn/interrupt':
        tid=p['threadId'];send(dict(id=i,result={}))
        notify('turn/completed',tid,turn={'id':'turn-'+tid,'status':'interrupted'})
        continue
    elif m is None and i in requests:
        assert f['result']['decision']=='accept'
        tid=requests.pop(i)
        notify('item/completed',tid,item={'type':'commandExecution','id':'cmd','command':'echo fixture','aggregatedOutput':'fixture\n','exitCode':0,'status':'completed'})
        notify('turn/completed',tid,turn={'id':'turn-'+tid,'status':'completed'})
        continue
    else:
        send(dict(id=i,error={'code':-32601,'message':'unknown method'}));continue
    send(dict(id=i,result=result))
