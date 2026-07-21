#!/bin/sh
dd if=/dev/zero bs=2048 count=1 2>/dev/null | tr '\000' A
dd if=/dev/zero bs=2048 count=1 2>/dev/null | tr '\000' B >&2
printf ' TS_SECRET_OVERSIZE 1.2.3\n'
