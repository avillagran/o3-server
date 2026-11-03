#!/usr/bin/env python3
"""Reject foreign native IDs and require the previous turn's transcript."""
import json, sys
claude = '-p' in sys.argv
seen_seed = claude and '--resume' in sys.argv
if seen_seed:
    assert sys.argv[sys.argv.index('--resume')+1] == 'claude-native'
for line in sys.stdin:
    f = json.loads(line)
    if claude:
        if f['type'] == 'control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}), flush=True)
            continue
        if f['type'] != 'user': continue
        text = f['message']['content'][0]['text']
    else:
        m, i = f.get('method'), f.get('id')
        if i is None: continue
        assert m != 'thread/fork'
        if m == 'initialize': result = {'capabilities':{}}
        elif m in ('thread/start', 'thread/resume'):
            if m == 'thread/resume': assert f['params']['threadId'] == 'codex-native'
            seen_seed = m == 'thread/resume'
            result = {'thread':{'id':'codex-native'}}
        elif m == 'turn/start':
            text = f['params']['input'][0]['text']
            result = {'turn':{'id':'turn'}}
        else: result = {}
        print(json.dumps({'id':i,'result':result}), flush=True)
        if m != 'turn/start': continue
    if text == 'later-proof':
        assert seen_seed, 'same-instance model change lost native context'
    if text.startswith('seed-proof'): seen_seed = True
    if 'fork-proof' in text:

        assert 'user: seed-proof' in text and 'context-proof' in text, text
        assert 'later-proof' not in text, text
        assert 'tool-output-proof' in text, text
        blocks = f['message']['content'] if claude else f['params']['input']
        images = [b for b in blocks if b['type'] in ('image','localImage')]
        assert not images, 'historical images must not be re-encoded as current uploads'
        references = [json.loads(line.split(': ', 1)[1]) for line in text.splitlines()
                      if line.startswith('Historical attachment (read when relevant): ')]
        assert len(references) == 1 and references[0]['available'] is True
        assert open(references[0]['path'], 'rb').read() == b'\x89PNG\r\n\x1a\n'
    if claude:
        print(json.dumps({'type':'system','subtype':'init','session_id':'claude-native'}), flush=True)
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'remembered-proof'}}}), flush=True)
        print(json.dumps({'type':'result','subtype':'success','session_id':'claude-native'}), flush=True)
    else:
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'codex-native','turnId':'turn','delta':'remembered-proof'}}), flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'codex-native','turn':{'id':'turn','status':'completed'}}}), flush=True)
