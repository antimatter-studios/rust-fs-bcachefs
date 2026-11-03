#!/usr/bin/env bash
# Exercise the real supervisor with external-tool stand-ins, without a VM or
# reference implementation. These are lifecycle checks, not format oracles.
#
# tests/scripts/guest-build-fixtures-guard-judges.sh runs this guard over a
# broken supervisor and a slowed host, through these settings. A run that names
# FIXTURE_GUARD_CASES ran part of the guard, so it never prints PASS:
#   FIXTURE_GUARD_SUPERVISOR  the script to run in place of the real supervisor
#   FIXTURE_GUARD_CASES       comma-separated labels of the cases to run
#   FIXTURE_TOOL_DELAY        seconds each stand-in sleeps before it starts
#   FIXTURE_GUARD_IDLE_LIMIT  seconds without progress that mean stuck
#   FIXTURE_GUARD_REAP_LIMIT  seconds a killed descendant may take to disappear
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "${FIXTURE_GUARD_SUPERVISOR:-$repo/scripts/guest-build-fixtures.sh}" "$repo" <<'PY'
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

script = Path(sys.argv[1])
support = Path(sys.argv[2]) / 'tests/scripts/support'
selected = os.environ.get('FIXTURE_GUARD_CASES')
def wanted(label):
    return not selected or label in selected.split(',')
stub = r'''import json, os, signal, sys, time
from pathlib import Path
time.sleep(float(os.environ.get('FIXTURE_TOOL_DELAY', 0)))
name, args = Path(sys.argv[0]).name, sys.argv[1:]
case = Path(os.environ['FIXTURE_CASE'])
root = Path(os.environ['BCACHEFS_REF_ROOT'])
out = Path(os.environ['BCACHEFS_FIXTURE_OUT'])
mode = os.environ['FIXTURE_MODE']
state = case / 'mount.json'
# Only so that nothing outlives a guard stopped part way: the guard judges stuck.
bound = float(os.environ['FIXTURE_STAND_IN_BOUND'])
def record(kind, **kw):
    with (case / 'events').open('a') as f:
        f.write(json.dumps(dict(kind=kind, pid=os.getpid(), sid=os.getsid(0), **kw)) + '\n')
def hold(kind):
    if mode == kind + '-hold':
        (case / 'ready').write_text(str(os.getpid()))
        while not (case / 'release').exists(): time.sleep(.01)
def image(arg):
    p = Path(arg)
    return p if p.is_relative_to(out) else root / arg.lstrip('/')
if name == 'mktemp':
    import tempfile
    p = tempfile.mkdtemp(prefix='fixture-build.', dir=root / 'var/tmp')
    record('allocation', scratch=p)
    if mode == 'foreign-mount':
        state.write_text(json.dumps(dict(mount=str(Path(p) / 'mnt'), pid=os.getpid())))
        record('foreign-mount')
    print(p)
elif name == 'bcachefs-ref':
    op = args[0]
    record('reference', operation=op)
    if op == 'format':
        for a in args:
            if a.startswith('--source='):
                src = image(a.split('=', 1)[1])
                assert src.is_dir(), 'formatter source is not visible inside chroot'
        if mode == 'format-fail': sys.exit(31)
        if mode == 'format-descendant-hold':
            # A child of the tool that ignores TERM and is not the supervisor's
            # to name: only the tool's session's process group can reach it.
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            child = os.fork()
            if child == 0:
                deadline = time.monotonic() + bound
                while time.monotonic() < deadline: time.sleep(.01)
                os._exit(0)
            signal.signal(signal.SIGTERM, signal.SIG_DFL)
            record('descendant', child=child)
            (case / 'ready').write_text(str(os.getpid()))
            while not (case / 'release').exists(): time.sleep(.01)
        hold('format')
    elif op == 'fusemount':
        img, mnt = image(args[-2].split(':')[0]), image(args[-1])
        assert img.exists() and (mnt.parent / '.owner').exists()
        record('fuse', mount=str(mnt), image=str(img))
        if mode == 'mount-fail': sys.exit(32)
        if mode == 'mount-refuse':
            (case / 'refused.img').write_bytes(img.read_bytes())
            (case / 'refused-path').write_text(str(img))
            print('Error: native filesystem refusal control', file=sys.stderr)
            sys.exit(42)
        import shutil
        for p in mnt.iterdir():
            if p.is_dir(): shutil.rmtree(p)
            else: p.unlink()
        if mode in ('mount-dead', 'mount-unrecorded-hold'):
            (case / 'mount-started').write_text(str(os.getpid()))
            deadline = time.monotonic() + bound
            while not (case / 'empty-mount-lookup').exists():
                assert time.monotonic() < deadline, 'mount lookup did not begin'
                time.sleep(.01)
        state.write_text(json.dumps(dict(mount=str(mnt), pid=os.getpid())))
        if mode == 'mount-dead':
            print('native daemon exited after attaching its mount', file=sys.stderr)
            sys.exit(43)
        while True: time.sleep(.01)
    elif op == 'show-super':
        clean = 0 if args[-1].endswith('/aged.img') and not (case / 'replayed').exists() else 1
        print('Clean: ', clean)
        print('Bucket size: 32k')
    elif op == 'fsck':
        if mode == 'check-fail': sys.exit(33)
        if '-y' in args and args[-1].endswith('/aged.img'): (case / 'replayed').touch()
    elif op == 'list' and len(args) > 2 and args[2] in ('reconcile_work', 'reconcile_work_phys'):
        # The supervisor refuses a reconcile.img whose lister shows no set key.
        print('u64s 5 type set 4096:128:U32_MAX len 0 ver 0')
    else: print('stand-in reference output')
elif name == 'mountpoint':
    mounted = state.exists() and json.loads(state.read_text())['mount'] == args[-1]
    if mounted:
        try: os.kill(json.loads(state.read_text())['pid'], 0)
        except ProcessLookupError:
            # Faithful ENOTCONN boundary: path-based readiness fails, but
            # the kernel mount record remains until explicitly detached.
            sys.exit(1)
    if mode == 'mount-hold' and mounted and not (case / 'signalled').exists():
        # The supervisor calls readiness only after recording the kernel ID.
        # The mount-unrecorded-hold control separately covers a signal before
        # the supervisor records an attached mount's identity.
        record('owned-mount-ready', mount=args[-1])
        (case / 'ready').write_text(str(os.getpid()))
        sys.exit(1)
    sys.exit(0 if mounted else 1)
elif name == 'findmnt':
    assert '--nocanonicalize' in args and '--kernel' in args
    target = args[args.index('--mountpoint') + 1]
    if not state.exists():
        started = case / 'mount-started'
        looked = case / 'empty-mount-lookup'
        # Only the first empty lookup races the daemon. A later one comes after
        # the detach, and must answer at once: nothing is going to attach.
        if mode in ('mount-dead', 'mount-unrecorded-hold') and started.exists() and not looked.exists():
            # Snapshot an empty table, then let the daemon attach before this
            # lookup returns. The supervisor must not mistake that old snapshot
            # for proof that its terminated child left no mount behind.
            pid = int(started.read_text())
            looked.touch()
            deadline = time.monotonic() + bound
            while not state.exists():
                assert time.monotonic() < deadline, 'daemon did not attach'
                time.sleep(.01)
            if mode == 'mount-dead':
                while True:
                    try: os.kill(pid, 0)
                    except ProcessLookupError: break
                    assert time.monotonic() < deadline, 'daemon did not exit'
                    time.sleep(.01)
                record('dead-after-empty-lookup')
            else:
                record('unrecorded-mount-ready')
                (case / 'ready').touch()
                while not (case / 'signalled').exists():
                    assert time.monotonic() < deadline, 'supervisor was not signalled'
                    time.sleep(.01)
        sys.exit(1)
    data = json.loads(state.read_text())
    assert data['mount'] == target, 'mount lookup targeted another invocation'
    identity = 1000
    if mode == 'mount-identity-changed':
        try: os.kill(data['pid'], 0)
        except ProcessLookupError: identity += 1
    print(identity)
elif name in ('fusermount3', 'umount'):
    if not state.exists(): sys.exit(1)
    data = json.loads(state.read_text())
    assert args[-1] == data['mount'], 'unmount targeted another invocation'
    assert Path(data['mount']).parent.exists(), 'scratch deleted before unmount'
    if args[0] == '-u':
        # A graceful unmount asks the tracked daemon to exit.
        os.kill(data['pid'], signal.SIGTERM)
    else:
        try: os.kill(data['pid'], 0)
        except ProcessLookupError: pass
        else: raise AssertionError('daemon not stopped and reaped before forced unmount')
    record('unmount', mount=data['mount'])
    if mode == 'unmount-fail': sys.exit(35)
    state.unlink()
elif name == 'cp':
    record('copy', destination=args[-1])
    if mode == 'copy-fail': sys.exit(34)
    hold('copy')
    import shutil
    shutil.copyfile(args[-2], args[-1])
elif name == 'python3':
    code = sys.stdin.read() if args[0] in ('-', '-c') else ''
    if args[0].endswith('/fixture-tree.py'):
        Path(args[1]).mkdir(parents=True)
        (Path(args[1]) / 'file').write_text('source tree')
    elif 'entries = []' in code:
        Path(args[2]).write_text(json.dumps(dict(entries=[])))
    elif 'before = json.load' in code:
        # Execute the real before/after assertion over the stand-in manifests.
        sys.argv = args
        exec(compile(code, '<manifest comparison>', 'exec'))
    elif ('for i in range(30000)' in code or 'def write(path, data)' in code
          or 'for i in range(1, 34)' in code):
        # The large, sparse and deep (#342) sets' generators: a source
        # directory the stand-in formatter can see.
        Path(args[1]).mkdir(parents=True)
    elif 'victim' in code and 'with open' in code:
        (Path(args[1]) / 'victim').write_text('victim')
        (Path(args[1]) / 'intact').write_text('intact')
elif name == 'truncate':
    for a in args[2:]: Path(a).write_bytes(b'stand-in image\n')
elif name == 'timeout':
    if args[0].startswith('--kill-after='): args = args[1:]
    os.execvp(args[1], args[1:])
elif name == 'sleep': time.sleep(.01)
elif name == 'sync': pass
elif name == 'rm':
    for a in args:
        if a.startswith('-'): continue
        p = Path(a)
        published = {
            'casefold.img',
            'encrypted.mounted.json',
            'encrypted-compressed.img', 'encrypted-compressed.json',
            'encrypted-wide-macs.img', 'encrypted-wide-macs.json',
            'encrypted-passphrase.mounted.json',
            'encrypted-unclean.img', 'encrypted-unclean.json',
            'encrypted-unclean.mounted.json',
            'encrypted-passphrase.img',
        }
        assert (p.is_relative_to(root / 'var/tmp') or
                (p.parent == out and p.name in published)), p
        assert p not in (root / 'var/tmp', root / 'var/tmp/age'), 'shared path deletion'
        if p.is_dir():
            if p.name.startswith('fixture-build.'):
                assert (p / '.owner').is_file(), 'unmarked root deletion'
                assert not state.exists(), 'scratch deleted while mounted'
                if mode == 'mount-refuse':
                    refused = Path((case / 'refused-path').read_text())
                    assert refused.read_bytes() == (case / 'refused.img').read_bytes(), 'refused image mutated'
                    record('refusal-preserved')
                record('remove', scratch=str(p))
            import shutil
            shutil.rmtree(p)
        elif p.exists(): p.unlink()
elif name in ('pkill', 'pgrep'):
    raise AssertionError('process-name matching can touch another invocation')
else: raise AssertionError(name)
'''

with tempfile.TemporaryDirectory(prefix='guest-fixture-cleanup-test.') as tmp:
    sandbox = Path(tmp)
    tools = sandbox / 'bin'
    tools.mkdir()
    executable = tools / 'stand-in'
    # Every stand-in is a new interpreter and that start is nearly all a tool
    # costs here, so no site-packages and no imports a tool does not use.
    executable.write_text('#!' + sys.executable + ' -IS\n' + stub)
    executable.chmod(0o755)
    for name in ('mktemp', 'bcachefs-ref', 'mountpoint', 'findmnt', 'fusermount3', 'umount',
                 'cp', 'python3', 'truncate', 'timeout', 'sleep', 'sync', 'rm', 'pkill', 'pgrep'):
        (tools / name).symlink_to(executable)
    watchdog = sandbox / 'watchdog.sh'
    watchdog.write_text('step() { :; }\n')
    study = sandbox / 'study.sh'
    study.write_text(': >"$BCACHEFS_FIXTURE_OUT/write-study-reached"\n')
    root = sandbox / 'ref'
    legacy = root / 'var/tmp/age'
    legacy.mkdir(parents=True)
    (legacy / 'keep.img').write_text('unmarked legacy image')
    fixtures = sandbox / 'fixtures'
    fixtures.mkdir()
    cached = fixtures / 'cached.json'
    cached.write_text('published manifest from another job')
    old_work = sandbox / 'fixtures-work'
    old_work.mkdir()
    (old_work / 'keep').touch()
    processes = []
    cases = []
    # Each wait follows the supervisor's trace, which grows with every command
    # it runs. However slow the host, a working supervisor keeps moving; one
    # that has stopped does not, and fails here whatever the load. MEASURED on
    # macOS arm64, 10 cores, load average 250 to 550, four of these guards beside
    # 60 spinners: the longest gap in a trace was 8 s, and the first mount took
    # 26 s to become ready where a fixed 15 s wait used to fail.
    IDLE_LIMIT = float(os.environ.get('FIXTURE_GUARD_IDLE_LIMIT', 60))
    # A supervisor that keeps tracing and never arrives still has to fail. The
    # longest case, a whole run, took 103 s in that measurement.
    CASE_LIMIT = 900
    # How many cases of each kind ran: a run cut short has fewer, and fails.
    ran = dict(status=0, signal=0, descendant=0)

    def signals_at_default():
        # A guard started in the background or under nohup inherits these ignored,
        # and a shell cannot trap what was ignored when it started.
        for sig in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, signal.SIG_DFL)

    def start(label, mode):
        case = sandbox / label
        case.mkdir()
        # The host's own setsid wins; tests/scripts/support is for a host with none.
        env = dict(os.environ, PATH=os.pathsep.join((str(tools), os.environ['PATH'], str(support))),
                   BCACHEFS_REF_ROOT=str(root), BCACHEFS_FIXTURE_OUT=str(fixtures),
                   BCACHEFS_FIXTURE_WATCHDOG=str(watchdog), BCACHEFS_FIXTURE_STUDY=str(study),
                   FIXTURE_CASE=str(case), FIXTURE_MODE=mode, WATCHDOG_PID='inherited-not-owned',
                   FIXTURE_STAND_IN_BOUND=str(CASE_LIMIT))
        with (case / 'log').open('w') as log:
            # Trace to a separate descriptor so native stderr captures keep
            # their exact bytes even when a command redirects stderr.
            env['BASH_XTRACEFD'] = str(log.fileno())
            proc = subprocess.Popen(['bash', '-x', str(script)], env=env, stdout=log,
                                    stderr=log, pass_fds=(log.fileno(),), start_new_session=True,
                                    preexec_fn=signals_at_default)
        processes.append(proc)
        cases.append(case)
        return proc, case

    def events(case):
        p = case / 'events'
        return [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []

    def in_session(pid, sid):
        # A pid alone may have been reused by now; a process left over from
        # this case is still in the session it started.
        try: return os.getsid(pid) == sid
        except (ProcessLookupError, PermissionError): return False

    def diagnose(case):
        lines = [f'--- what {case.name} last traced']
        lines += (case / 'log').read_text(errors='replace').splitlines()[-40:]
        lines += ['--- its last events'] + [json.dumps(e) for e in events(case)[-20:]]
        table = subprocess.run(['ps', '-axo', 'pid,ppid,pgid,stat,etime,command'],
                               capture_output=True, text=True).stdout.splitlines()
        lines += ['--- its processes still running'] + [l[:200] for l in table if str(sandbox) in l][:40]
        return '\n'.join(lines)

    def wait_for(case, what, done):
        began = moved = time.monotonic()
        seen = None
        while not done():
            now = time.monotonic()
            mark = [p.stat().st_size if p.exists() else 0 for p in (case / 'log', case / 'events')]
            if mark != seen: seen, moved = mark, now
            if now - moved > IDLE_LIMIT:
                raise AssertionError(f'{case.name}: waited {now - began:.0f}s for {what}, '
                                     f'with no progress for {now - moved:.0f}s\n{diagnose(case)}')
            if now - began > CASE_LIMIT:
                raise AssertionError(f'{case.name}: waited {now - began:.0f}s for {what}, '
                                     f'and the supervisor is still running commands\n{diagnose(case)}')
            time.sleep(.01)

    def await_ready(proc, case):
        def ready():
            if (case / 'ready').exists(): return True
            assert proc.poll() is None, (f'{case.name}: the supervisor exited with status '
                                         f'{proc.returncode} before its child was ready\n{diagnose(case)}')
            return False
        wait_for(case, "the held child's ready file", ready)

    def finish(proc, case, status, retained=False):
        wait_for(case, f'the supervisor to exit with status {status}', lambda: proc.poll() is not None)
        actual = proc.returncode
        assert actual == status, (actual, status, (case / 'log').read_text())
        records = events(case)
        roots = [Path(e['scratch']) for e in records if e['kind'] == 'allocation']
        assert len(roots) == 1, 'invocation must allocate one private root'
        scratch = roots[0]
        # A failed ownership assertion must retain the supervisor's reason for
        # keeping scratch; TemporaryDirectory removes this stand-in log on exit.
        # Keep success quiet and bound failure output without dropping events.
        diagnostic = dict(case=case.name, events=records,
                          supervisor_tail='\n'.join((case / 'log').read_text().splitlines()[-80:]))
        assert scratch.exists() == retained, ('owned root lifecycle', diagnostic)
        assert (case / 'mount.json').exists() == retained, ('mount lifecycle', diagnostic)
        for e in records:
            if e['kind'] in ('reference', 'copy'):
                assert e['sid'] == e['pid'], ('owned child does not lead its own session', e)
                assert not in_session(e['pid'], e['sid']), ('owned child survived cleanup', e)
            if e['kind'] == 'descendant':
                # Killed is not yet gone: init still has to reap an orphan.
                end = time.monotonic() + float(os.environ.get('FIXTURE_GUARD_REAP_LIMIT', 60))
                while in_session(e['child'], e['sid']):
                    assert time.monotonic() < end, ('descendant survived cleanup', e)
                    time.sleep(.01)
                ran['descendant'] += 1
        assert (legacy / 'keep.img').read_text() == 'unmarked legacy image'
        assert (old_work / 'keep').exists()
        assert cached.read_text() == 'published manifest from another job'
        if not retained: assert any(e['kind'] == 'remove' for e in records)
        ran['signal' if status > 128 else 'status'] += 1
        return scratch

    passed = False
    try:
        for mode, status in [('format-fail', 31), ('check-fail', 1),
                             ('mount-fail', 1), ('copy-fail', 34)]:
            if not wanted(mode): continue
            proc, case = start(mode, mode)
            finish(proc, case, status)
        for mode, native_status in [('mount-refuse', 42), ('mount-dead', 43)]:
            if not wanted(mode): continue
            proc, case = start(mode, mode)
            finish(proc, case, 1)
            log = (case / 'log').read_text()
            assert f'the reference mount exited: status={native_status}' in log, log
            records = events(case)
            assert sum(e['kind'] == 'fuse' for e in records) == 1, 'refused mount was retried'
            if mode == 'mount-refuse':
                assert 'Error: native filesystem refusal control' in log, log
                captures = [Path(e['destination']) for e in records
                            if e['kind'] == 'copy' and e['destination'].endswith('.stderr.txt')]
                assert len(captures) == 1, 'native stderr was not retained'
                assert captures[0].read_text() == 'Error: native filesystem refusal control\n'
                assert any(e['kind'] == 'refusal-preserved' for e in records)
                assert not any(e['kind'] == 'unmount' for e in records)
            else:
                assert any(e['kind'] == 'dead-after-empty-lookup' for e in records), 'race was not exercised'
                assert sum(e['kind'] == 'unmount' for e in records) == 1, 'dead endpoint was not detached once'
        if wanted('foreign-mount'):
            proc, case = start('foreign-mount', 'foreign-mount')
            finish(proc, case, 1, retained=True)
            assert any(e['kind'] == 'foreign-mount' for e in events(case))
            assert not any(e['kind'] in ('fuse', 'unmount') for e in events(case)), 'foreign mount was adopted'
        if wanted('changed-mount'):
            proc, case = start('changed-mount', 'mount-identity-changed')
            finish(proc, case, 1, retained=True)
            assert 'mount identity changed; refusing to detach' in (case / 'log').read_text()
            assert not any(e['kind'] == 'unmount' for e in events(case))
        for sig in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            for mode in ('format-hold', 'format-descendant-hold', 'mount-hold',
                         'mount-unrecorded-hold', 'copy-hold'):
                if not wanted(f'{sig.name}-{mode}'): continue
                proc, case = start(f'{sig.name}-{mode}', mode)
                await_ready(proc, case)
                if mode == 'mount-hold':
                    assert any(e['kind'] == 'owned-mount-ready' for e in events(case))
                    assert not any(e['kind'] == 'unmount' for e in events(case))
                if mode == 'mount-unrecorded-hold':
                    assert any(e['kind'] == 'unrecorded-mount-ready' for e in events(case))
                (case / 'signalled').touch()
                proc.send_signal(sig)
                finish(proc, case, 128 + sig)
        if wanted('concurrent'):
            first, a = start('concurrent-a', 'format-hold')
            second, b = start('concurrent-b', 'format-hold')
            await_ready(first, a)
            await_ready(second, b)
            a_root, b_root = events(a)[0]['scratch'], events(b)[0]['scratch']
            assert a_root != b_root
            first.send_signal(signal.SIGTERM)
            finish(first, a, 143)
            assert second.poll() is None and Path(b_root).exists(), 'other invocation touched'
            (b / 'release').touch()
            finish(second, b, 0)
            for name in ('default.img', 'default.json', 'large.img', 'large.json',
                         'aged.img', 'aged-unclean.img', 'aged.json', 'aged-unclean.json',
                         'crc128.img', 'crc128.json', 'poison.img', 'probe.img',
                         'probe.kvdb.txt', 'encrypted.img', 'multi-0.img', 'multi-1.img',
                         'encrypted-compressed.img', 'encrypted-compressed.json',
                         'encrypted-wide-macs.img', 'encrypted-wide-macs.json',
                         'encrypted-unclean.img', 'encrypted-unclean.json',
                         'encrypted-unclean.mounted.json',
                         'encrypted-passphrase.img', 'encrypted-passphrase.mounted.json',
                         'casefold.img', 'casefold.json', 'reference-help.txt',
                         'reference-version.txt', 'write-study-reached'):
                assert (fixtures / name).exists(), f'published fixture lost: {name}'
        if wanted('busy-unmount'):
            proc, case = start('busy-unmount', 'unmount-fail')
            finish(proc, case, 1, retained=True)
        if not selected:
            assert ran == dict(status=10, signal=16, descendant=3), ('cases that ran', ran)
        passed = True
    finally:
        for proc in processes:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
        # A failure must not leave held stand-ins behind to load the next run.
        if not passed:
            for case in cases:
                for sid in {e['sid'] for e in events(case) if e['kind'] in ('reference', 'copy')}:
                    try: os.killpg(sid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError): pass
if selected: print('PARTIAL  ran only', selected)
else: print('PASS  fixture builds retain published output and clean only owned scratch after tools and mounts stop')
PY
