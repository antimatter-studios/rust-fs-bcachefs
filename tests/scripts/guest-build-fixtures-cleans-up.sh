#!/usr/bin/env bash
# Exercise the real supervisor with external-tool stand-ins, without a VM or
# reference implementation. These are lifecycle checks, not format oracles.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/scripts/guest-build-fixtures.sh" <<'PY'
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

script = Path(sys.argv[1])
stub = r'''import json, os, shutil, signal, sys, tempfile, time
from pathlib import Path
name, args = Path(sys.argv[0]).name, sys.argv[1:]
case = Path(os.environ['FIXTURE_CASE'])
root = Path(os.environ['BCACHEFS_REF_ROOT'])
out = Path(os.environ['BCACHEFS_FIXTURE_OUT'])
mode = os.environ['FIXTURE_MODE']
state = case / 'mount.json'
def record(kind, **kw):
    with (case / 'events').open('a') as f:
        f.write(json.dumps(dict(kind=kind, pid=os.getpid(), **kw)) + '\n')
def hold(kind):
    if mode == kind + '-hold':
        (case / 'ready').write_text(str(os.getpid()))
        while not (case / 'release').exists(): time.sleep(.01)
def image(arg):
    p = Path(arg)
    return p if p.is_relative_to(out) else root / arg.lstrip('/')
if name == 'mktemp':
    p = tempfile.mkdtemp(prefix='fixture-build.', dir=root / 'var/tmp')
    record('allocation', scratch=p)
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
        for p in mnt.iterdir():
            if p.is_dir(): shutil.rmtree(p)
            else: p.unlink()
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
        # Daemon attachment alone can precede that ownership handoff; a signal
        # then correctly refuses to detach an as-yet unrecorded mount.
        record('owned-mount-ready', mount=args[-1])
        (case / 'ready').write_text(str(os.getpid()))
        sys.exit(1)
    sys.exit(0 if mounted else 1)
elif name == 'findmnt':
    assert '--nocanonicalize' in args and '--kernel' in args
    target = args[args.index('--mountpoint') + 1]
    if not state.exists(): sys.exit(1)
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
    elif 'for i in range(30000)' in code or 'def write(path, data)' in code:
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
        assert p.is_relative_to(root / 'var/tmp') or p == out / 'casefold.img', p
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
    executable.write_text('#!' + sys.executable + '\n' + stub)
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

    def start(label, mode):
        case = sandbox / label
        case.mkdir()
        env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                   BCACHEFS_REF_ROOT=str(root), BCACHEFS_FIXTURE_OUT=str(fixtures),
                   BCACHEFS_FIXTURE_WATCHDOG=str(watchdog), BCACHEFS_FIXTURE_STUDY=str(study),
                   FIXTURE_CASE=str(case), FIXTURE_MODE=mode, WATCHDOG_PID='inherited-not-owned')
        with (case / 'log').open('w') as log:
            # Trace to a separate descriptor so native stderr captures keep
            # their exact bytes even when a command redirects stderr.
            env['BASH_XTRACEFD'] = str(log.fileno())
            proc = subprocess.Popen(['bash', '-x', str(script)], env=env, stdout=log,
                                    stderr=log, pass_fds=(log.fileno(),), start_new_session=True)
        processes.append(proc)
        return proc, case

    def events(case):
        p = case / 'events'
        return [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []

    def await_ready(proc, case):
        end = time.monotonic() + 15
        while time.monotonic() < end:
            if (case / 'ready').exists(): return
            assert proc.poll() is None, (case / 'log').read_text()
            time.sleep(.01)
        raise AssertionError('owned child did not become ready')

    def finish(proc, case, status, retained=False):
        actual = proc.wait(timeout=120)
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
                try: os.kill(e['pid'], 0)
                except ProcessLookupError: pass
                else: raise AssertionError('owned child survived cleanup')
        assert (legacy / 'keep.img').read_text() == 'unmarked legacy image'
        assert (old_work / 'keep').exists()
        assert cached.read_text() == 'published manifest from another job'
        if not retained: assert any(e['kind'] == 'remove' for e in records)
        return scratch

    try:
        for mode, status in [('format-fail', 31), ('check-fail', 1),
                             ('mount-fail', 1), ('copy-fail', 34)]:
            proc, case = start(mode, mode)
            finish(proc, case, status)
        for mode, native_status in [('mount-refuse', 42), ('mount-dead', 43)]:
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
                assert any(e['kind'] == 'unmount' for e in records), 'dead endpoint was not detached'
        proc, case = start('changed-mount', 'mount-identity-changed')
        finish(proc, case, 1, retained=True)
        assert 'mount identity changed; refusing to detach' in (case / 'log').read_text()
        assert not any(e['kind'] == 'unmount' for e in events(case))
        for sig in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            for mode in ('format-hold', 'mount-hold', 'copy-hold'):
                proc, case = start(f'{sig.name}-{mode}', mode)
                await_ready(proc, case)
                if mode == 'mount-hold':
                    assert any(e['kind'] == 'owned-mount-ready' for e in events(case))
                    assert not any(e['kind'] == 'unmount' for e in events(case))
                (case / 'signalled').touch()
                proc.send_signal(sig)
                finish(proc, case, 128 + sig)
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
                     'casefold.img', 'casefold.json', 'reference-help.txt',
                     'reference-version.txt', 'write-study-reached'):
            assert (fixtures / name).exists(), f'published fixture lost: {name}'
        proc, case = start('busy-unmount', 'unmount-fail')
        finish(proc, case, 1, retained=True)
    finally:
        for proc in processes:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
print('PASS  fixture builds retain published output and clean only owned scratch after tools and mounts stop')
PY
