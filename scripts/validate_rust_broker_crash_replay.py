#!/usr/bin/env python3
"""Qualify native broker crash/replay under distinct UIDs in a disposable root VM.

Run with --bin pointing to the packaged storage service. This fixture changes
ownership only within its temporary directory; it requires an isolated root
host and unused UIDs 45000 through 45002. No network endpoint is contacted.
"""
import argparse
from hashlib import sha256
from pathlib import Path
import json
import os
import pwd
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time

BROKER, CLIENT, UNKNOWN = 45000, 45001, 45002
REQUEST = '0123456789abcdef0123456789abcdef'
RECEIVER = 'receiver:native'
SOURCE = 'harness.test'

def drop(uid, gid, groups=()):
    def configure():
        os.setgroups(groups)
        os.setgid(gid)
        os.setuid(uid)
    return configure

def exact(connection, size):
    chunks = bytearray()
    while len(chunks) < size:
        chunk = connection.recv(size-len(chunks))
        assert chunk
        chunks.extend(chunk)
    return chunks

def client(path, operation, mode):
    if mode == 'direct':
        for flags in (os.O_RDONLY, os.O_WRONLY):
            try:
                os.close(os.open(path, flags))
            except PermissionError:
                continue
            raise AssertionError("client opened broker DB")
        return
    body = json.dumps(dict(version=1, request_id=REQUEST, receiver_id=RECEIVER, operation=operation), separators=(',',':')).encode()
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(4)
        connection.connect(path)
        pid, uid, gid = struct.unpack('3i',connection.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
        assert pid > 0 and (uid,gid)==(BROKER,BROKER)
        if mode == 'no-reply':
            assert connection.recv(1)==b''
            return
        frame = struct.pack('>I',len(body))+body
        if mode == 'incomplete':
            connection.sendall(frame[:5])
            return
        connection.sendall(frame)
        connection.shutdown(socket.SHUT_WR)
        if mode == 'lost-reply':
            return
        length, = struct.unpack('>I',exact(connection,4))
        reply = json.loads(exact(connection,length))
        assert reply['identity']==dict(version=1,request_id=REQUEST,receiver_id=RECEIVER,request_sha256=list(sha256(body).digest()))
        assert connection.recv(1)==b''
        print(json.dumps(reply['result']),flush=True)

def run(binary):
    if os.geteuid() != 0:
        raise SystemExit('Run only as root in a disposable qualification VM')
    for uid in (BROKER, CLIENT, UNKNOWN):
        try:
            pwd.getpwuid(uid)
        except KeyError:
            continue
        raise SystemExit('Qualification UID already belongs to an account')
    with tempfile.TemporaryDirectory(prefix='agenturm-native-broker-') as scratch:
        root = Path(scratch)
        root.chmod(0o755)
        data = root/'private-data'
        transport = root/'transport'
        data.mkdir()
        transport.mkdir()
        os.chown(data, BROKER, BROKER)
        os.chown(transport, BROKER, CLIENT)
        data.chmod(0o700)
        transport.chmod(0o2710)
        path = transport/'agenturm-native-storage-v1.sock'
        db = data/'history.sqlite'
        args = [str(binary),'--data-root',data,'--transport-dir',transport,'--receiver-id',RECEIVER,'--client-uid',str(CLIENT),'--client-gid',str(CLIENT),'--native-sources',SOURCE]
        def start():
            service = subprocess.Popen(args,preexec_fn=drop(BROKER,BROKER))
            for _ in range(200):
                if path.exists() and stat.S_IMODE(path.stat().st_mode)==0o660 and db.exists():
                    try:
                        result = request(dict(op='source_cursor',source_id=SOURCE))
                    except subprocess.CalledProcessError:
                        time.sleep(.04)
                        continue
                    return service
                assert service.poll() is None
                time.sleep(.04)
            service.kill()
            service.wait()
            raise AssertionError('native listener did not become ready')
        def request(operation, mode='reply', uid=CLIENT, gid=CLIENT, groups=()):
            result = subprocess.check_output([sys.executable,__file__,db if mode=='direct' else path,mode,json.dumps(operation)],
                preexec_fn=drop(uid,gid,groups), text=True)
            return json.loads(result) if result else None
        def cursor():
            return request(dict(op='source_cursor',source_id=SOURCE))['Ok']['cursor']
        def batch(event, previous, next_cursor):
            document = dict(source_id=SOURCE,expected_cursor=previous,next_cursor=next_cursor,observations=[
                dict(event_id=event,revision=0,identity=dict(scope='session',session_id='session-one',worker_id=None,turn_id=None,operation_id=None),
                    observed_at_unix_ns=1,kind=dict(kind='session_started'))])
            identity = sha256(json.dumps(document,separators=(',',':')).encode()).hexdigest()
            return dict(op='admit',batch=dict(request_id=identity,**document))
        first = batch('event-one',None,'cursor-1')
        service = start()
        assert cursor() is None
        competing_transport = root/'competing-transport'
        competing_transport.mkdir()
        os.chown(competing_transport, BROKER, CLIENT)
        competing_transport.chmod(0o2710)
        competing_args = list(args)
        competing_args[competing_args.index('--transport-dir') + 1] = competing_transport
        competing = subprocess.Popen(competing_args, preexec_fn=drop(BROKER,BROKER),
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        competing.communicate(timeout=5)
        assert competing.returncode != 0, 'competing broker obtained the same writer lease'
        assert cursor() is None
        request({},'direct')
        request({},'no-reply',UNKNOWN,CLIENT)
        request({},'no-reply',CLIENT,BROKER,(CLIENT,))
        assert request(dict(op='source_cursor',source_id='harness.denied'))=={'Err':'unauthorized'}
        request(first,'incomplete')
        service.kill()
        assert service.wait()==-signal.SIGKILL
        assert path.exists()
        service = start()
        assert cursor() is None
        assert request(first)=={'Ok':{'result':'committed','inserted':1}}
        service.kill()
        assert service.wait()==-signal.SIGKILL and path.exists()
        service = start()
        assert request(first)=={'Ok':{'result':'replay'}}
        assert request(batch('event-two','cursor-1','cursor-2'))=={'Ok':{'result':'committed','inserted':1}}
        assert request(first)=={'Ok':{'result':'replay'}}
        assert cursor()=='cursor-2'
        third = batch('event-three','cursor-2','cursor-3')
        request(third,'lost-reply')
        for _ in range(100):
            if cursor() == 'cursor-3':
                break
            time.sleep(.02)
        else:
            raise AssertionError('unacknowledged complete frame was not durably committed')
        service.kill()
        assert service.wait()==-signal.SIGKILL and path.exists()
        service = start()
        assert cursor()=='cursor-3'
        assert request(third)=={'Ok':{'result':'replay'}}
        assert cursor()=='cursor-3'
        request({},'direct')
        service.terminate()
        assert service.wait(timeout=3)==0
        assert not path.exists()
        import sqlite3
        with sqlite3.connect(db) as connection:
            assert connection.execute('SELECT count(*) FROM agenturm_observations').fetchone()==(3,)
        print('PASS native broker distinct UID/GID, direct DB denial, peer and source rejection, three SIGKILL restarts, lost reply after durable admission, competing writer denial, three durable observations and exact replay',flush=True)

if len(sys.argv)==4:
    client(sys.argv[1],json.loads(sys.argv[3]),sys.argv[2])
else:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin', type=Path, required=True)
    run(parser.parse_args().bin.resolve())
