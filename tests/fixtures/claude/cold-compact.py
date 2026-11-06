#!/usr/bin/env python3
import json, sys
from pathlib import Path
assert '--tools' in sys.argv and sys.argv[sys.argv.index('--tools') + 1] == ''
assert '--no-session-persistence' not in sys.argv
turn = 0
for line in sys.stdin:
    f = json.loads(line)
    if f['type'] == 'control_request':
        print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}), flush=True)
    elif f['type'] == 'user':
        text = f['message']['content'][0]['text']
        turn += 1
        if turn == 1:
            assert 'historical-proof' in text and 'Summarize' in text
        else:
            assert turn == 2 and text == '/compact'
            Path('compact-finished').write_text('native compact follows summary')
        print(json.dumps({'type':'system','subtype':'init','session_id':'cold-fixture'}), flush=True)
        print(json.dumps({'type':'result','subtype':'success','num_turns':1}), flush=True)
