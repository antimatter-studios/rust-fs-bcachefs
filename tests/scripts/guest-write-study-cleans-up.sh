#!/usr/bin/env bash
# The real guest script, with external tools replaced by tiny stand-ins.
# No reference source, VM, mount privileges, or filesystem fixtures are used.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "${STUDY_GUARD_SUPERVISOR:-$repo/scripts/guest-write-study.sh}" "$repo" <<'PY'
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

script = Path(sys.argv[1])
support = Path(sys.argv[2]) / 'tests/scripts/support'
sys.path.insert(0, str(support))
from supervisor_guard import SupervisorProgress, events, in_session, signals_at_default, trace_progress_into

selected = os.environ.get('STUDY_GUARD_CASES')
def wanted(label):
    return not selected or label in selected.split(',')
# These stand-ins model tool boundaries, not the reference filesystem format.
# Persistent trees make the script's unchanged fixture operations executable.
stub = r'''import json, os, signal, sys, time
from pathlib import Path
time.sleep(float(os.environ.get('STUDY_TOOL_DELAY', 0)))
name = Path(sys.argv[0]).name
args = sys.argv[1:]
case = Path(os.environ['STUDY_CASE'])
root = Path(os.environ['BCACHEFS_REF_ROOT'])
mode = os.environ['STUDY_MODE']
state = case / 'mount.json'
bound = float(os.environ['STUDY_STAND_IN_BOUND'])
def record(kind, **kw):
    with (case / 'events').open('a') as f:
        f.write(json.dumps(dict(kind=kind, pid=os.getpid(), sid=os.getsid(0), **kw)) + '\n')
def pause():
    deadline = time.monotonic() + bound
    while not (case / 'release').exists():
        assert time.monotonic() < deadline, 'stand-in hold exceeded its bound'
        time.sleep(.02)
def tree(img): return Path(str(img) + '.tree')
def image(arg): return root / arg.lstrip('/')
if name == 'bcachefs-ref':
    op = args[0]
    if op == 'format':
        img = image(args[-1])
        record('format', image=str(img), scratch=str(img.parent))
        if mode == 'format-fail': sys.exit(31)
        if mode in ('hold', 'descendant-hold'):
            if mode == 'descendant-hold':
                child = os.fork()
                if child == 0:
                    pause()
                    os._exit(0)
                record('descendant', child=child)
            (case / 'ready').write_text(str(os.getpid()))
            pause()
        tree(img).mkdir(exist_ok=True)
    elif op == 'fusemount':
        import shutil
        img, mnt = image(args[-2]), image(args[-1])
        record('fuse', mount=str(mnt), image=str(img))
        if mode == 'mount-fail': sys.exit(32)
        # The private mount directory is the only location this fake mount owns.
        shutil.rmtree(mnt)
        shutil.copytree(tree(img), mnt)
        state.write_text(json.dumps(dict(image=str(img), mount=str(mnt), pid=os.getpid())))
        if mode == 'mount-hold':
            (case / 'ready').write_text(str(os.getpid()))
        deadline = time.monotonic() + bound
        while time.monotonic() < deadline: time.sleep(1)
    elif op == 'fsck' and mode == 'check-fail': sys.exit(33)
    elif op == 'kvdb':
        # Model the printed CLI result from completed CI run 38763620048,
        # not reference implementation source. The expected refusal must
        # remain fixture evidence without aborting the fixture producer.
        command = sys.stdin.read().strip()
        record('kvdb', command=command)
        if command.startswith('get inodes '):
            assert '--rw' not in args
            print('  bi_compression=0')
        elif command.startswith('update inodes ') and command.endswith(' bi_compression=2'):
            assert '--rw' in args
            print("Error: bch_inode_v3 has no field 'bi_compression'")
            sys.exit(1)
        else: raise AssertionError(('unexpected offline editor command', command))
    else: print('stand-in oracle output')
elif name == 'mountpoint':
    mounted = state.exists() and json.loads(state.read_text())['mount'] == args[-1]
    # Keep the real script waiting for mount readiness to exercise its live
    # FUSE child on signal; the unmount stand-in still observes the resource.
    if mode == 'mount-hold' and mounted and not (case / 'signalled').exists():
        sys.exit(1)
    sys.exit(0 if mounted else 1)
elif name in ('fusermount3', 'umount'):
    import shutil
    if not state.exists(): sys.exit(1)
    data = json.loads(state.read_text())
    assert args[-1] == data['mount'], 'attempt to unmount another invocation'
    try: os.kill(data['pid'], 0)
    except ProcessLookupError: pass
    else: raise AssertionError('FUSE child was not killed and reaped before unmount')
    mnt = Path(data['mount'])
    assert mnt.parent.exists(), 'scratch was removed before unmount'
    record('unmount', mount=str(mnt))
    shutil.rmtree(tree(data['image']))
    shutil.copytree(mnt, tree(data['image']))
    state.unlink()
elif name == 'cp':
    import shutil
    src, dst = map(Path, args[-2:])
    record('copy', source=str(src), destination=str(dst))
    if mode == 'copy-fail': sys.exit(34)
    if mode == 'copy-hold':
        (case / 'ready').write_text(str(os.getpid()))
        pause()
    shutil.copyfile(src, dst)
    if tree(src).exists() and str(dst).startswith(str(root)):
        shutil.copytree(tree(src), tree(dst), dirs_exist_ok=True)
elif name == 'truncate':
    Path(args[-1]).write_bytes(b'stand-in image\n')
elif name == 'seq':
    if args == ['1', '60']: print('\n'.join(map(str, range(1, 61))))
    else: print(args[0] if len(args) > 1 else '1')
elif name == 'sleep':
    time.sleep(.02)
elif name in ('sync', 'python3'):
    pass
elif name == 'stat':
    # GNU's stat -c, as the guest has it: BSD's stat has no -c.
    assert args[0] == '-c', args
    for path in args[2:]:
        st = os.stat(path)
        print(args[1].replace('%i', str(st.st_ino)).replace('%s', str(st.st_size)).replace('%n', path))
elif name == 'rm':
    import subprocess
    # The obsolete script's blanket fixture deletion must fail safely, too.
    for arg in args:
        if not arg.startswith('-'):
            p = Path(arg)
            assert p.is_relative_to(root) or p.is_relative_to(Path(os.environ['BCACHEFS_STUDY_OUT'])), p
            assert p.name not in ('write-study', 'cached.txt'), 'blanket fixture/work deletion'
            if p.name.startswith('write-study.') and p.is_dir():
                assert (p / '.owner').is_file(), 'unmarked root deletion'
                assert not state.exists(), 'scratch removed while mounted'
                record('cleanup', scratch=str(p), arguments=args)
                refused = case / 'directory-refused'
                if mode == 'cleanup-directory-refusal' and not refused.exists():
                    # Inject only the external cleanup error printed by completed
                    # CI job 116524001271. Do not infer a filesystem diagnosis.
                    record('directory-refusal', mount=str(p / 'mnt'))
                    refused.touch()
                    print(f"rm: cannot remove '{p / 'mnt'}': Is a directory", file=sys.stderr)
                    sys.exit(1)
    # Keep the real option, symlink and directory semantics: in particular,
    # neither plain rm nor rm -f may recursively delete a directory.
    sys.exit(subprocess.run([os.environ['STUDY_REAL_RM'], *args]).returncode)
elif name in ('pkill', 'pgrep'):
    raise AssertionError('process-name matching can affect another invocation')
else: raise AssertionError(name)
'''

with tempfile.TemporaryDirectory(prefix='guest-write-study-test.') as tmp:
    sandbox = Path(tmp)
    tools = sandbox / 'bin'
    tools.mkdir()
    executable = tools / 'stand-in'
    executable.write_text('#!' + sys.executable + ' -IS\n' + stub)
    executable.chmod(0o755)
    for name in ('bcachefs-ref', 'mountpoint', 'fusermount3', 'umount', 'cp',
                 'truncate', 'seq', 'sleep', 'sync', 'python3', 'stat', 'rm', 'pkill', 'pgrep'):
        (tools / name).symlink_to(executable)
    watchdog = sandbox / 'watchdog.sh'
    watchdog.write_text('step() { :; }\n')
    root = sandbox / 'ref'
    old = root / 'var/tmp/write-study'
    old.mkdir(parents=True)
    (old / 'keep.img').write_text('unmarked old work')
    fixtures = sandbox / 'fixtures/write-study'
    fixtures.mkdir(parents=True)
    cached = fixtures / 'cached.txt'
    cached.write_text('cached oracle output')
    processes = []
    cases = []
    IDLE_LIMIT = float(os.environ.get('STUDY_GUARD_IDLE_LIMIT', 60))
    CASE_LIMIT = 900
    progress = SupervisorProgress(sandbox, IDLE_LIMIT, CASE_LIMIT)
    wait_for = progress.wait_for
    await_ready = progress.await_ready
    ran = dict(status=0, signal=0, descendant=0)

    def start(label, mode):
        case = sandbox / label
        case.mkdir()
        # The host's own setsid wins; tests/scripts/support is for a host with none.
        env = dict(os.environ, PATH=os.pathsep.join((str(tools), os.environ['PATH'], str(support))),
                   BCACHEFS_REF_ROOT=str(root), BCACHEFS_STUDY_OUT=str(fixtures),
                   BCACHEFS_STUDY_WATCHDOG=str(watchdog), STUDY_CASE=str(case),
                   STUDY_MODE=mode, STUDY_REAL_RM=shutil.which('rm'),
                   WATCHDOG_PID='inherited-not-owned', STUDY_STAND_IN_BOUND=str(CASE_LIMIT))
        with (case / 'log').open('w') as log:
            trace_progress_into(env, log)
            proc = subprocess.Popen(['bash', str(script)], env=env, stdout=log,
                                    stderr=log, pass_fds=(log.fileno(),), start_new_session=True,
                                    preexec_fn=signals_at_default)
        processes.append(proc)
        cases.append(case)
        return proc, case

    def finish(proc, case, status):
        wait_for(case, f'the supervisor to exit with status {status}', lambda: proc.poll() is not None)
        actual = proc.returncode
        assert actual == status, (actual, status, (case / 'log').read_text())
        records = events(case)
        allocations = {e['scratch'] for e in records if e['kind'] == 'format'}
        assert allocations, 'the formatter was never reached'
        assert all(not Path(p).exists() for p in allocations), 'owned scratch survived'
        assert not (case / 'mount.json').exists(), 'owned mount survived'
        for event in records:
            if event['kind'] in ('format', 'fuse', 'copy'):
                assert event['sid'] == event['pid'], ('owned child does not lead its own session', event)
                assert not in_session(event['pid'], event['sid']), ('owned child survived cleanup', event)
            if event['kind'] == 'descendant':
                end = time.monotonic() + float(os.environ.get('STUDY_GUARD_REAP_LIMIT', 60))
                while in_session(event['child'], event['sid']):
                    assert time.monotonic() < end, ('descendant survived cleanup', event)
                    time.sleep(.01)
                ran['descendant'] += 1
        assert (old / 'keep.img').read_text() == 'unmarked old work'
        assert cached.read_text() == 'cached oracle output'
        ran['signal' if status > 128 else 'status'] += 1
        return allocations

    passed = False
    try:
        # A directory named like the real failed mount must survive rm -f.
        # This control would fail with the old recursively-deleting stand-in.
        control = sandbox / 'rm-control'
        control.mkdir()
        mount = root / 'var/tmp/rm-control/mnt'
        mount.mkdir(parents=True)
        payload = mount / 'keep'
        payload.write_text('directory contents')
        env = dict(os.environ, STUDY_CASE=str(control), BCACHEFS_REF_ROOT=str(root),
                   BCACHEFS_STUDY_OUT=str(fixtures), STUDY_MODE='rm-control',
                   STUDY_REAL_RM=shutil.which('rm'), STUDY_STAND_IN_BOUND=str(CASE_LIMIT), LC_ALL='C')
        refusal = subprocess.run([str(tools / 'rm'), '-f', '--', str(mount)],
                                 env=env, text=True, capture_output=True)
        assert refusal.returncode == 1, (refusal.returncode, refusal.stderr)
        # GNU rm says "Is a directory", BSD rm "is a directory".
        assert 'is a directory' in refusal.stderr.lower(), refusal.stderr
        assert payload.read_text() == 'directory contents'
        subprocess.run([str(tools / 'rm'), '-rf', '--', str(mount.parent)],
                       env=env, check=True)
        assert not mount.parent.exists(), 'recursive rm control did not remove its root'
        for mode, status in [('format-fail', 31), ('mount-fail', 1),
                             ('check-fail', 1), ('copy-fail', 34)]:
            if not wanted(mode): continue
            proc, case = start(mode, mode)
            finish(proc, case, status)
        for sig in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            for mode in ('hold', 'descendant-hold', 'mount-hold', 'copy-hold'):
                if not wanted(f'{sig.name}-{mode}'): continue
                proc, case = start(f'{sig.name}-{mode}', mode)
                await_ready(proc, case)
                # Signal only the supervisor; it must kill/reap its child.
                (case / 'signalled').touch()
                proc.send_signal(sig)
                finish(proc, case, 128 + sig)
        if wanted('concurrent'):
            first, a = start('concurrent-a', 'hold')
            second, b = start('concurrent-b', 'hold')
            await_ready(first, a)
            await_ready(second, b)
            a_root = events(a)[0]['scratch']
            b_root = events(b)[0]['scratch']
            assert a_root != b_root
            first.send_signal(signal.SIGTERM)
            finish(first, a, 143)
            assert second.poll() is None and Path(b_root).exists(), 'other invocation was touched'
            (b / 'release').touch()
            finish(second, b, 0)
            assert (fixtures / 'base.img').read_bytes() == b'stand-in image\n'
            assert (fixtures / 'xcollide-reset.img').exists(), 'success stopped before the last fixture'
            checker_output = (fixtures / 'xcollide-reset.fsck.txt').read_text().strip()
            assert checker_output == 'stand-in oracle output', checker_output
            commands = [e['command'] for e in events(b) if e['kind'] == 'kvdb']
            assert len(commands) == 2, ('offline editor probe was not completed', commands)
            position = commands[0].removeprefix('get inodes ')
            assert commands == [f'get inodes {position}',
                                f'update inodes {position} bi_compression=2'], commands
            transcript = (fixtures / 'kvdb-options.txt').read_text()
            assert f'## get inodes {position}\n  bi_compression=0\n' in transcript, transcript
            assert (f'## update inodes {position} bi_compression=2\n'
                    "Error: bch_inode_v3 has no field 'bi_compression'\nexit 1\n") in transcript, transcript
            assert sorted(p.name for p in (root / 'var/tmp').iterdir()) == ['write-study']
        # All existing lifecycle and refusal controls above must finish before
        # this red case. A completed producer must also clean up when rm reports
        # the observed mount-directory error; status 1 or surviving scratch fails.
        if wanted('cleanup-directory-refusal'):
            proc, case = start('cleanup-directory-refusal', 'cleanup-directory-refusal')
            finish(proc, case, 0)
            records = events(case)
            assert any(e['kind'] == 'directory-refusal' for e in records), records
            assert any(e['kind'] == 'copy' and
                       e['destination'] == str(fixtures / 'xcollide-reset.img')
                       for e in records), 'cleanup failed before publishing the final fixture'
            assert (fixtures / 'xcollide-reset.img').exists()
            checker_output = (fixtures / 'xcollide-reset.fsck.txt').read_text().strip()
            assert checker_output == 'stand-in oracle output', checker_output
        if not selected:
            assert ran == dict(status=6, signal=13, descendant=3), ('cases that ran', ran)
        passed = True
    finally:
        # The regression itself owns only these process groups and this sandbox.
        for proc in processes:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
        if not passed:
            for case in cases:
                for sid in {event['sid'] for event in events(case)
                            if event['kind'] in ('format', 'fuse', 'copy')}:
                    try: os.killpg(sid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError): pass
if selected: print('PARTIAL  ran only', selected)
else: print('PASS  write-study removes only owned scratch after success, failure and signals')
PY
