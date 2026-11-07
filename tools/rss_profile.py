"""Real desktop probe. Private records stay in memory and the isolated disk copy."""
import shutil
from runtime_safety import require_private_runtime
import argparse, hashlib, json, os, pathlib, queue, signal, sqlite3, subprocess, threading, time, tempfile, urllib.request

def run(memory):
    require_private_runtime()
    parser = argparse.ArgumentParser()
    parser.add_argument('binary', nargs='?', default='/var/tmp/o3-build/main/release/o3-server')
    parser.add_argument('--profile', required=True)
    parser.add_argument('--bridge-public', action='store_true', help='public relay, QUIC client and real Claude turn (copies local credentials)')
    parser.add_argument('--settle-seconds', type=float, default=3)
    parser.add_argument('--probe', default='/var/tmp/o3-build/main/release/examples/bridge_probe')
    parser.add_argument('--diagnostics', action='store_true')
    parser.add_argument('--output', default='docs/rss-measurement.json')
    args = parser.parse_args()
    if args.bridge_public:
        assert (pathlib.Path.home()/'.claude/.credentials.json').is_file(), 'local Claude credentials required'
        assert pathlib.Path(args.probe).is_file(), 'build release example bridge_probe first'
    binary = pathlib.Path(args.binary).resolve()
    assert '/release/' in str(binary), 'benchmark requires a release binary'
    root = pathlib.Path('/var/tmp/o3-build/real-opt')
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    # A consistent read-only backup, never open the source in writable mode.
    source = pathlib.Path(args.profile).resolve() / 'userdata/o3.sqlite'
    home = pathlib.Path(tempfile.mkdtemp(prefix='run-', dir=root))
    profile = home/'t3'
    (profile/'userdata').mkdir(parents=True, mode=0o700)
    with sqlite3.connect(source.as_uri()+'?mode=ro', uri=True) as src, sqlite3.connect(profile/'userdata/o3.sqlite') as dst:
        src.backup(dst)
        tid, count = dst.execute("SELECT thread_id,count(*) FROM records WHERE kind='visibleTurnItems' GROUP BY thread_id ORDER BY count(*) DESC LIMIT 1").fetchone()
        counts = {k:n for k,n in dst.execute('SELECT kind,count(*) FROM records GROUP BY kind')}
    src.close()
    dst.close()
    (home/'bin').mkdir()
    shutil.copy2(binary, home/'bin/t3')
    env = {**os.environ, 'HOME':str(home), 'T3CODE_HOME':str(profile), 'TMPDIR':'/var/tmp/o3-build/tmp',
           'CODEX_HOME':str(home/'codex'), 'CLAUDE_CONFIG_DIR':str(home/'claude'),
           'PATH':str(home/'bin')+':/usr/bin:/bin'}
    for name in ('CONFIG','DATA','STATE','RUNTIME'):
        env['XDG_'+name+'_HOME' if name != 'RUNTIME' else 'XDG_RUNTIME_DIR'] = str(home/name.lower())
    if args.bridge_public:
        (home/'claude').mkdir(mode=0o700)
        shutil.copyfile(pathlib.Path.home()/'.claude/.credentials.json', home/'claude/.credentials.json')
        (home/'claude/.credentials.json').chmod(0o600)
        env['PATH'] = str(home/'bin')+':'+os.environ['PATH']
        (home/'server').symlink_to(profile)
    client = os.environ.get('O3_DESKTOP_BIN','/var/tmp/o3-build/e2e/desktop-target/debug/omarchy-t3-client')
    diagnostics = []
    phase = 'startup'
    process = subprocess.Popen([str(binary),'serve','--port','0']+(['--bridge-bind','0.0.0.0:0','--bridge-relay','public'] if args.bridge_public else []),env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True)
    def stderr_reader():
        for line in process.stderr:
            try:
                v=json.loads(line)
                if 'rust_live_bytes' in v or 'rust_large_live_bytes' in v: diagnostics.append({'phase':phase, **v})
            except ValueError: pass
    threading.Thread(target=stderr_reader,daemon=True).start()
    watch = None
    probe = None
    def sample(label):
        nonlocal phase
        phase=label
        if args.diagnostics:
            process.send_signal(signal.SIGUSR1)
            time.sleep(.1)
        v=memory(process.pid)
        # smaps_rollup has no mappings or private paths.
        v['AnonHugePages']=next(int(line.split()[1]) for line in pathlib.Path(f'/proc/{process.pid}/smaps_rollup').read_text().splitlines() if line.startswith('AnonHugePages:'))
        v['Pss_Anon']=next(int(line.split()[1]) for line in pathlib.Path(f'/proc/{process.pid}/smaps_rollup').read_text().splitlines() if line.startswith('Pss_Anon:'))
        return v
    try:
        deadline = time.monotonic()+90
        while not (profile/'omabridge/control.sock').exists():
            assert process.poll() is None and time.monotonic()<deadline, 'isolated startup failed'
            time.sleep(.02)
        runtime=json.loads((profile/'userdata/server-runtime.json').read_text())
        with urllib.request.urlopen(f"http://127.0.0.1:{runtime['port']}/.well-known/t3/environment", timeout=90) as response:
            assert response.status == 200
        time.sleep(35 if args.bridge_public else .2)
        idle=sample('idle')
        started=time.monotonic()
        snap=json.loads(subprocess.check_output([client,'snapshot'],env=env,stderr=subprocess.DEVNULL,timeout=90))
        snapshot_ms=round((time.monotonic()-started)*1000,2)
        del snap
        after_snapshot=sample('snapshot')
        watch=subprocess.Popen([client,'watch'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
        events=queue.Queue()
        def reader():
            for line in watch.stdout:
                try:
                    v=json.loads(line)
                    # Retain only event names, never private message content.
                    events.put((v.get('type'), v.get('data',{}).get('connected') is True))
                except ValueError: pass
        threading.Thread(target=reader,daemon=True).start()
        for v in ({'op':'visible','value':True},{'op':'select','environment':'local','thread':tid,'serial':1}):
            watch.stdin.write(json.dumps(v)+'\n');watch.stdin.flush()
        deadline=time.monotonic()+90
        messages = connected = False
        while time.monotonic()<deadline:
            event, ready = events.get(timeout=max(.1,deadline-time.monotonic()))
            messages = messages or event == 'messages'
            connected = connected or (event == 'live-state' and ready)
            if messages and connected: break
        else: raise RuntimeError('thread view timeout')
        time.sleep(1)
        opened=sample('desktop_thread')
        watch.terminate();watch.wait(timeout=10);watch=None
        bridged = {}
        if args.bridge_public:
            probe=subprocess.Popen([args.probe,str(home)],env={**env,'O3_RSS_THREAD':tid},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            for stage in ('ready','shell','thread','churn','turn'):
                deadline=time.monotonic()+120
                while not (home/stage).exists():
                    assert probe.poll() is None and time.monotonic()<deadline, 'bridged '+stage+' failed'
                    time.sleep(.02)
                bridged[stage]=sample(stage)
            probe.wait(timeout=15)
            assert probe.returncode == 0, 'bridged probe failed'
            probe=None
        time.sleep(args.settle_seconds)
        disconnected=sample('disconnected')
        max_hwm=max(v['VmHWM'] for v in [idle,after_snapshot,opened,disconnected,*bridged.values()])
        result={'measured_at_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
                'profile_counts':counts,'biggest_thread_items':count,'idle_kib':idle,'after_snapshot_kib':after_snapshot,'thread_open_kib':opened,
                'after_disconnect_kib':disconnected,'snapshot_ms':snapshot_ms,'bridge_enabled':True,'client':'real omarchy-t3-client snapshot/watch + Rust QUIC' if args.bridge_public else 'real omarchy-t3-client snapshot/watch',
                'measurement':'Linux /proc/PID/status, isolated SQLite backup; server only (provider subprocess excluded)','heap_samples':diagnostics,
                'lifetime_hwm_kib':max_hwm,'under_50_mb':max_hwm*1024<50_000_000,
                'relay':'public' if args.bridge_public else 'none','bridged_kib':bridged,'native_turn':args.bridge_public,'bridged_unary_calls':100 if args.bridge_public else 0,'idle_settle_seconds':35 if args.bridge_public else .2,'disconnect_settle_seconds':args.settle_seconds,
                'under_35_mb_idle':idle['VmRSS']*1024<35_000_000,
                'disconnect_within_3_mb_idle':(disconnected['VmRSS']-idle['VmRSS'])*1024<3_000_000,'under_500_ms_snapshot':snapshot_ms<500,
                'allocator_env':{k:v for k,v in env.items() if k.startswith(('MIMALLOC_','MALLOC_'))}}
        pathlib.Path(args.output).write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result,indent=2))
        assert result['under_50_mb'] and result['under_35_mb_idle'] and result['under_500_ms_snapshot'] and result['disconnect_within_3_mb_idle'], 'real-profile budget exceeded'
    finally:
        if probe is not None: probe.terminate();probe.wait(timeout=10)
        if watch is not None: watch.terminate();watch.wait(timeout=10)
        process.terminate();process.wait(timeout=10)
        # Retain the isolated measurement workspace for review; never traverse it for cleanup.
