#!/usr/bin/env python3
"""Native goal protocol peer with independently gated automatic turns."""
import json
import pathlib
import sys
import threading
import time

lock = threading.Lock()
threads = {}
sequence = 0

def send(value):
    with lock:
        print(json.dumps(value), flush=True)

def notify(method, tid, **params):
    send({"method": method, "params": {"threadId": tid, **params}})

def gate(root, name):
    deadline = time.monotonic() + 20
    while not (root / name).exists():
        if time.monotonic() > deadline:
            return False
        time.sleep(.01)
    return True

def started(tid, turn):
    state = threads[tid]
    state['running'] = turn
    notify('turn/started', tid, turn={'id': turn, 'status': 'inProgress'})

def completed(tid, turn, status='completed'):
    threads[tid]['running'] = None
    notify('turn/completed', tid, turn={'id': turn, 'status': status})

def next_turn(tid):
    state = threads[tid]
    root = state['root']
    if not gate(root, 'next'):
        return
    if not state['goal'] or state['goal']['status'] != 'active':
        return
    state['automatic_turns'] += 1
    turn = 'automatic-' + tid + ('' if state['automatic_turns'] == 1 else '-'+str(state['automatic_turns']))
    started(tid, turn)
    if (root / 'hold-second').exists() and not gate(root, 'second-complete'):
        return
    if state['running'] != turn:
        return
    notify('item/agentMessage/delta', tid, turnId=turn, itemId='second', delta='SECOND\n\n')
    state['goal']['status'] = 'complete'
    notify('thread/goal/updated', tid, goal=state['goal'])
    completed(tid, turn)

def late_turn(tid):
    if gate(threads[tid]['root'], 'orphan'):
        started(tid, 'late-' + tid)

def delayed_activation(tid, request, snapshot):
    if (threads[tid]['root'] / 'clear-during-activation').exists():
        if gate(threads[tid]['root'], 'clear-native'):
            threads[tid]['goal'] = None
            notify('thread/goal/cleared', tid)
            if gate(threads[tid]['root'], 'activation-reply'):
                send({'id': request, 'result': {'goal': snapshot}})
        return
    if gate(threads[tid]['root'], 'activation-reply'):
        # Model a delayed mutation/acknowledgement racing a later Stop request.
        threads[tid]['goal']['status'] = 'active'
        notify('thread/goal/updated', tid, goal=threads[tid]['goal'])
        send({'id': request, 'result': {'goal': snapshot}})

if '-p' in sys.argv:
    goal = None
    for line in sys.stdin:
        frame = json.loads(line)
        if frame['type'] == 'control_request':
            send({'type':'control_response','response':{'subtype':'success','request_id':frame['request_id']}})
            continue
        if frame['type'] != 'user':
            continue
        text = frame['message']['content'][0]['text']
        send({'type':'system','subtype':'init','session_id':'claude-native-goal'})
        def synthetic(reply):
            send({'type':'assistant','parent_tool_use_id':None,'message':{'model':'<synthetic>','content':[{'type':'text','text':reply}]}})
        if text == '/goal clear':
            goal = None
            synthetic('Goal cleared: previous goal')
        elif text.startswith('/goal '):
            goal = {'objective':text[len('/goal '):], 'checks':0}
            synthetic('Goal set: '+goal['objective'])
        elif text == '/goal':
            synthetic('No goal set' if goal is None else 'Goal active: '+goal['objective']+' ('+str(goal['checks'])+' turns)')
        elif text == 'child-only':
            send({'type':'assistant','parent_tool_use_id':'child','message':{'model':'haiku','content':[{'type':'text','text':'child response'}]}})
        else:
            if goal:
                goal['checks'] += 1
                send({'type':'user','isSynthetic':True,'parent_tool_use_id':None,'message':{'content':'Stop hook feedback:\n['+goal['objective']+']: continue until verified'}})
            send({'type':'assistant','parent_tool_use_id':None,'message':{'model':'haiku','content':[{'type':'text','text':'CLAUDE_GOAL_PROOF'}]}})
            send({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'CLAUDE_GOAL_PROOF'}}})
        failed = text == 'fail'
        send({'type':'result','subtype':'error_during_execution' if failed else 'success','is_error':failed,'terminal_reason':'api_error' if failed else 'completed','result':'fixture failure' if failed else '', 'session_id':'claude-native-goal','num_turns':1})
    sys.exit(0)

for line in sys.stdin:
    frame = json.loads(line)
    method, params, request = frame.get('method'), frame.get('params', {}), frame.get('id')
    tid = params.get('threadId')
    if tid in threads:
        root = threads[tid]['root']
        with (root / 'goal-trace.jsonl').open('a') as trace:
            trace.write(json.dumps({'method': method, 'params': {k: params[k] for k in ('status','objective','origin','turnId','expectedTurnId','input','model','effort','collaborationMode') if k in params}}) + '\n')
        if method.startswith('thread/goal/') and (root / 'no-goal-api').exists():
            send({'id': request, 'error': {'code': -32601, 'message': 'Native goal API unavailable'}})
            continue
    if method == 'initialize':
        result = {'userAgent': 'codex/0.166.0 fixture'}
    elif method == 'initialized':
        continue
    elif method == 'account/read':
        result = {'account': None, 'requiresOpenaiAuth': False}
    elif method == 'model/list':
        result = {'data': [{'model': 'goal-fixture', 'displayName': 'Goal fixture', 'isDefault': True}]}
    elif method in ('thread/start', 'thread/resume'):
        if method == 'thread/start':
            sequence += 1
            tid = 'native-' + str(sequence)
            threads[tid] = {'root': pathlib.Path(params['cwd']), 'goal': None, 'running': None, 'initial_turns':0, 'automatic_turns':0}
        result = {'thread': {'id': tid}}
    elif method == 'thread/goal/get':
        result = {'goal': threads[tid]['goal']}
    elif method == 'thread/goal/clear':
        assert params.get('origin') == 'user', 'clear must retain explicit command provenance'
        previous = threads[tid]['goal']
        threads[tid]['goal'] = None
        notify('thread/goal/cleared', tid)
        result = {'cleared': previous is not None}
    elif method == 'thread/goal/set':
        state = threads[tid]
        if params.get('objective') or params.get('status') == 'active':
            assert params.get('origin') == 'user', 'creating or activating a goal needs user provenance'
        if params.get('objective') == 'reject-this-objective':
            send({'id':request,'error':{'code':-32602,'message':'goal objective must not be empty'}})
            continue
        if params.get('status') == 'active' and (state['root'] / 'activation-fails').exists():
            send({'id':request,'error':{'code':-32602,'message':'Activation refused'}})
            continue
        goal = state['goal']
        previous_status = goal['status'] if goal else None
        if goal is None:
            if not params.get('objective'):
                send({'id': request, 'error': {'code': -32602, 'message': 'No goal'}})
                continue
            goal = {'objective': params['objective'], 'tokensUsed': 0, 'timeUsedSeconds': 0, 'tokenBudget': None, 'status': 'paused'}
        goal.update({k: params[k] for k in ('objective', 'status') if k in params})
        state['goal'] = goal
        notify('thread/goal/updated', tid, goal=goal)
        if goal['status'] == 'active' and ((state['root'] / 'hold-activation').exists() or (state['root'] / 'clear-during-activation').exists()):
            threading.Thread(target=delayed_activation, args=(tid,request,json.loads(json.dumps(goal))), daemon=True).start()
            continue
        send({'id': request, 'result': {'goal': goal}})
        if goal['status'] == 'paused' and previous_status == 'active' and (state['root'] / 'late-after-pause').exists():
            threading.Thread(target=late_turn, args=(tid,), daemon=True).start()
        if goal['status'] == 'active' and not (state['root'] / 'no-next').exists():
            threading.Thread(target=next_turn, args=(tid,), daemon=True).start()
        continue
    elif method == 'turn/start':
        if (threads[tid]['root'] / 'start-fails').exists():
            send({'id':request,'error':{'code':-32602,'message':'Native start refused'}})
            continue
        state=threads[tid]
        state['initial_turns'] += 1
        turn = 'initial-' + tid + ('' if state['initial_turns'] == 1 else '-'+str(state['initial_turns']))
        send({'id': request, 'result': {'turn': {'id': turn}}})
        started(tid, turn)
        notify('item/agentMessage/delta', tid, turnId=turn, itemId='first', delta='FIRST\n\n')
        completed(tid, turn)
        continue
    elif method == 'turn/interrupt':
        turn = params['turnId']
        if threads[tid]['running'] != turn:
            send({'id': request, 'error': {'code': -32602, 'message': 'Turn already ended'}})
        else:
            send({'id': request, 'result': {}})
            completed(tid, turn, 'interrupted')
        continue
    elif method == 'turn/steer':
        assert params['expectedTurnId'] == threads[tid]['running']
        result = {'turnId': params['expectedTurnId']}
    elif method == 'thread/unsubscribe':
        send({'id': request, 'result': {'status': 'unsubscribed'}})
        notify('thread/closed', tid)
        continue
    else:
        result = {}
    if request is not None:
        send({'id': request, 'result': result})
