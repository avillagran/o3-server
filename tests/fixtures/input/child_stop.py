#!/usr/bin/env python3
import json, sys
started = False
silent = False
def emit(v): print(json.dumps(v), flush=True)
for line in sys.stdin:
    f=json.loads(line)
    if f['type']=='control_request':
        r=f['request']; assert r['subtype']!='interrupt'
        emit({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}})
        if r['subtype']=='stop_task' and not silent:
            assert r['task_id']=='child-task'
            emit({'type':'system','subtype':'task_notification','task_id':'child-task','tool_use_id':'child-tool','status':'stopped','summary':'Stopped child'})
    elif f['type']=='user':
        emit({'type':'system','subtype':'init','session_id':'owner'})
        if not started:
            silent = f['message']['content'][0]['text'] == 'silent'
            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'child-tool','name':'Agent','input':{'prompt':'work','description':'Child'}}]}})
            emit({'type':'system','subtype':'task_started','task_id':'child-task','tool_use_id':'child-tool','task_type':'local_agent'})
        else: emit({'type':'result','subtype':'success','session_id':'owner'})
        started=True
