#!/bin/sh
case "$1" in
  unknown)
    printf 'release-current\n'
    ;;
  fail)
    printf 'TS_SECRET_STDERR\n' >&2
    exit 7
    ;;
  *)
    printf 'tool 1.2.3\n'
    ;;
esac
