#!/usr/bin/env python3
import json, sys
started = False
late = False
def emit(v): print(json.dumps(v), flush=True)
for line in sys.stdin:
    f=json.loads(line)
    if f['type']=='control_request':
        r=f['request']; assert r['subtype'] not in ('interrupt', 'stop_task')
        emit({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}})
    elif f['type']=='user':
        emit({'type':'system','subtype':'init','session_id':'owner'})
        if not started:
            late = f['message']['content'][0]['text'] == 'late'
            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'child-tool','name':'Agent','input':{'prompt':'work','description':'Child'}}]}})
            emit({'type':'system','subtype':'task_started','task_id':'child-task','tool_use_id':'child-tool','task_type':'local_agent'})
            emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'child-tool','content':'background-task-returned'}]}})
        else:
            if late:
                emit({'type':'control_response','response':{'subtype':'success','request_id':'stop-child:child-task'}})
            # A real child terminal notification, never a fabricated stop timeout.
            emit({'type':'system','subtype':'task_notification','task_id':'child-task','tool_use_id':'child-tool','status':'completed','summary':'Child completed'})
            emit({'type':'result','subtype':'success','session_id':'owner'})
        started=True
