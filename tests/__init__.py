"""Promotes `tests/` to a regular package so `tests.fixtures.router_stub`
imports deterministically and is not shadowed by a same-named PEP 420
namespace package from a third-party install on the host.
"""
