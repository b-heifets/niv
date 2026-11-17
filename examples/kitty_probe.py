#!/usr/bin/env python3
"""Check native Kitty graphics support through the CURRENT terminal or Rex pane.

No third-party dependencies; POSIX/macOS/Linux only. Must be run in a TTY.
"""
import os
import select
import sys
import termios
import time
import tty

QUERY = b'\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c'


def main():
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise SystemExit('Run inside an interactive terminal, not via a pipe.')
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    reply = bytearray()
    try:
        tty.setraw(fd)
        os.write(sys.stdout.fileno(), QUERY)
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            ready, _, _ = select.select([fd], [], [], max(0.0, deadline - time.monotonic()))
            if not ready:
                break
            reply.extend(os.read(fd, 4096))
            if b'\x1b[' in reply and (b'c' in reply.split(b'\x1b[')[-1]):
                break
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)
    sys.stdout.write('\r\n')
    supported = b'\x1b_Gi=31;OK\x1b\\' in reply
    print('Kitty graphics query:', 'SUPPORTED' if supported else 'NO CONFIRMED RESPONSE')
    print('Raw terminal reply:', repr(bytes(reply)))
    if not supported:
        print('A missing response may mean the terminal or multiplexer does not forward Kitty graphics.')


if __name__ == '__main__':
    main()
