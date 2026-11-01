#!/usr/bin/env python3
import json, sys, os
from pathlib import Path
with Path('starts').open('a') as f: f.write(str(os.getpid())+'\n')
def send(v): print(json.dumps(v), flush=True)
for line in sys.stdin:
    f=json.loads(line)
    if f['type']=='control_request':
        send({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}})
    elif f['type']=='user':
        send({'type':'system','subtype':'init','session_id':'native-session'})
        send({'type':'control_request','request_id':'permission','request':{'subtype':'can_use_tool','tool_name':'Bash','input':{'command':f['message']['content'][0]['text']}}})
    elif f['type']=='control_response':
        send({'type':'result','subtype':'success','session_id':'native-session'})
