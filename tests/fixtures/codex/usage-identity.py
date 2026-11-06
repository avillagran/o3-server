#!/usr/bin/env python3
"""Synthetic Codex peer: account identity comes from the same native probe as limits."""
import json,os,sys
account=os.environ['O3_TEST_ACCOUNT']
for line in sys.stdin:
    request=json.loads(line);method=request.get('method')
    if method=='initialize': result={'userAgent':'codex/0.166.0'}
    elif method=='initialized': continue
    elif method=='model/list': result={'data':[]}
    elif method=='account/read': result={'account':{'type':'chatgpt','email':account+'@example.test'}}
    elif method=='account/rateLimits/read': result={'accountId':'account-'+account,'rateLimits':{'primary':{'usedPercent':37,'windowDurationMins':300}}}
    else: raise RuntimeError('Unexpected inference call: '+str(method))
    print(json.dumps({'id':request['id'],'result':result}),flush=True)
