#!/usr/bin/python3
"""Native MCP launch peer: exercise inherited context and root binding, without inference."""
import json, pathlib, sys, urllib.request

claude = '-p' in sys.argv
entries = {}
counter = 0
config = None
if claude:
    config = json.load(open(sys.argv[sys.argv.index('--mcp-config') + 1]))['mcpServers']['t3-code']

def emit(value):
    print(json.dumps(value), flush=True)

def launch(config, root):
    headers = dict(config.get('headers', config.get('http_headers')))
    headers.update({'Content-Type': 'application/json', 'Accept': 'application/json'})
    def request(method, params):
        body = json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params}).encode()
        with urllib.request.urlopen(urllib.request.Request(config['url'], data=body, headers=headers), timeout=10) as response:
            if method == 'initialize':
                headers['Mcp-Session-Id'] = response.headers['Mcp-Session-Id']
            result = json.load(response)
            assert 'result' in result, result
            return result['result']
    request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {}, 'clientInfo': {'name': 'launch-peer', 'version': '1'}})
    params = {'name': 't3_thread_launch', 'arguments': {'title': 'Native child', 'message': 'launch-child', 'clientRequestId': 'native-launch'}}
    first = request('tools/call', params)
    replay = request('tools/call', params)
    assert not first['isError'] and not replay['isError'], (first, replay)
    assert first['structuredContent']['threadId'] == replay['structuredContent']['threadId']
    (root / 'native-launch.json').write_text(json.dumps(first['structuredContent']))

for line in sys.stdin:
    value = json.loads(line)
    if claude:
        if value['type'] == 'control_request':
            emit({'type': 'control_response', 'response': {'request_id': value['request_id'], 'subtype': 'success'}})
            continue
        if value['type'] != 'user':
            continue
        text = value['message']['content'][0]['text']
        root = pathlib.Path.cwd()
        native = 'native'
        emit({'type': 'system', 'subtype': 'init', 'session_id': native})
    else:
        method, ident, params = value.get('method'), value.get('id'), value.get('params', {})
        if method == 'thread/start':
            counter += 1
            native = 'native-' + str(counter)
            entries[native] = (pathlib.Path(params['cwd']), params['config']['mcp_servers.t3-code'])
            emit({'id': ident, 'result': {'thread': {'id': native}}})
            continue
        if method != 'turn/start':
            if ident is not None:
                emit({'id': ident, 'result': {}})
            continue
        native = params['threadId']
        root, config = entries[native]
        text = params['input'][0]['text']
        emit({'id': ident, 'result': {'turn': {'id': 'turn-' + native}}})
    if 'launch-parent' in text:
        launch(config, root)
    else:
        assert 'launch-child' in text, text
        (root / 'native-child-proof').write_text('ready')
    if claude:
        emit({'type': 'result', 'subtype': 'success', 'session_id': native})
    else:
        emit({'method': 'turn/completed', 'params': {'threadId': native, 'turn': {'id': 'turn-' + native, 'status': 'completed'}}})
