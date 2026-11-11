#!/usr/bin/python3
"""Native MCP attachment peer: prepare/upload/send/self-default/discard across sessions."""
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
        body = json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode()
        with urllib.request.urlopen(urllib.request.Request(config['url'],data=body,headers=headers),timeout=10) as response:
            if method == 'initialize': headers['Mcp-Session-Id'] = response.headers['Mcp-Session-Id']
            result = json.load(response)
            assert 'result' in result, result
            return result['result']
    def init():
        request('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'attachment-peer','version':'1'}})
    def tool(name, arguments):
        result=request('tools/call',{'name':name,'arguments':arguments})
        assert not result['isError'], result
        return result['structuredContent']
    init()
    data=b'{"cells":[{"cell_type":"markdown","source":"Document token NATIVE-731"}]}'
    prepared=tool('t3_attachment_prepare_upload',{'upload':{'type':'file','name':'report.ipynb','mimeType':'application/json','sizeBytes':len(data)}})
    from urllib.parse import urljoin
    with urllib.request.urlopen(urllib.request.Request(urljoin(config['url'],prepared['relativeUrl']),data=data,method='POST'),timeout=10) as response:
        assert response.status==204
    sent=tool('t3_thread_send_attachments',{'attachments':[{'type':'file','id':prepared['attachmentId'],'name':'report.ipynb','mimeType':'application/json','sizeBytes':len(data)}]})
    # Transport sessions are not upload ownership. A fresh one from this native caller can discard.
    headers.pop('Mcp-Session-Id');init()
    tool('t3_attachment_discard',{'attachmentId':prepared['attachmentId']})
    (root/'native-attachment.json').write_text(json.dumps(sent))

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
        if method not in ('turn/start', 'turn/steer'):
            if ident is not None:
                emit({'id': ident, 'result': {}})
            continue
        native = params['threadId']
        root, config = entries[native]
        text = params['input'][0]['text']
        emit({'id': ident, 'result': {'turn': {'id': 'turn-' + native}}})
    if 'attachment-parent' in text:
        launch(config, root)
        continue
    else:
        content=value['message']['content'] if claude else params['input']
        assert content[0]['text'].startswith('[User attached one or more files without additional text.'), content
        reference=content[1]['text']
        original=json.JSONDecoder().raw_decode(reference.split('available at ',1)[1])[0]
        sidecar=json.JSONDecoder().raw_decode(reference.split('text extracted to: ',1)[1])[0]
        assert pathlib.Path(original).is_file()
        assert 'Document token NATIVE-731' in pathlib.Path(sidecar).read_text()
        (root/'native-attachment-proof').write_text('ready')
    if claude:
        emit({'type': 'result', 'subtype': 'success', 'session_id': native})
    else:
        emit({'method': 'turn/completed', 'params': {'threadId': native, 'turn': {'id': 'turn-' + native, 'status': 'completed'}}})
