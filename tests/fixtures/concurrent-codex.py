#!/usr/bin/env python3
"""Credential-free gated app-server peer shared by the two differential servers."""
import json, os, sys, threading, time
from pathlib import Path
if '--version' in sys.argv:
    print('codex-cli 0.166.0')
    sys.exit(0)
lock = threading.Lock()
root = Path(os.environ['T3CODE_HOME'])
turns = 0
active = {}
def send(v):
    with lock:
        print(json.dumps(v), flush=True)
def notify(method, tid, **p):
    send(dict(method=method, params=dict(threadId=tid, **p)))
def finish(tid, turn, n):
    end = time.monotonic() + 60
    while not (root / ('release-' + str(n))).exists():
        if not active.get(turn): return
        if time.monotonic() > end: return
        time.sleep(.01)
    if active.pop(turn, False):
        notify('turn/completed', tid, turn=dict(id=turn, status='completed', items=[], error=None, startedAt=int(time.time()), completedAt=int(time.time())))
for line in sys.stdin:
    f = json.loads(line)
    m, p, i = f.get('method'), f.get('params') or {}, f.get('id')
    if i is None: continue
    with (root / 'peer-methods.log').open('a') as log: log.write(str(m) + '\n')
    if m == 'initialize': result = dict(userAgent='fixture/0.166.0', codexHome=str(root / 'codex'), platformFamily='unix', platformOs='linux')
    elif m == 'account/read': result = dict(account=dict(type='apiKey'), requiresOpenaiAuth=False)
    elif m == 'model/list': result = dict(data=[dict(id='fixture', model='fixture', displayName='Fixture', description='Fixture', hidden=False, supportedReasoningEfforts=[], defaultReasoningEffort='low', inputModalities=['text'], supportsPersonality=False, isDefault=True)], nextCursor=None)
    elif m == 'skills/list': result = dict(data=[])
    elif m in ('thread/start', 'thread/resume'):
        tid = p.get('threadId', 'native-fixture')
        result = dict(thread=dict(id=tid, projectId=None, sessionId='fixture-session', preview='', ephemeral=False, modelProvider='openai', createdAt=int(time.time()), updatedAt=int(time.time()), status=dict(type='idle'), path=None, cwd=str(root), cliVersion='0.166.0', source='appServer', turns=[]))
        result.update(approvalPolicy='never', approvalsReviewer='user', cwd=str(root), model='fixture', modelProvider='openai', sandbox=dict(type='dangerFullAccess'))
    elif m == 'turn/start':
        turns += 1
        tid, turn = p['threadId'], 'turn-' + str(turns)
        active[turn] = True
        result = dict(turn=dict(id=turn, status='inProgress', items=[], error=None, startedAt=int(time.time()), completedAt=None))
        send(dict(id=i, result=result))
        notify('turn/started', tid, turn=result['turn'])
        (root / ('started-' + str(turns))).touch()
        threading.Thread(target=finish, args=(tid, turn, turns), daemon=True).start()
        continue
    elif m == 'turn/interrupt':
        turn = p['turnId']; active.pop(turn, None)
        send(dict(id=i, result={}))
        notify('turn/completed', p['threadId'], turn=dict(id=turn, status='interrupted', items=[], error=None))
        continue
    elif m in ('thread/read',): result = dict(thread=dict(id=p['threadId'], turns=[]))
    else: result = {}
    send(dict(id=i, result=result))
