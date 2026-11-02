"""Scheduling primitives for commands sharing an Execution Pod."""
from concurrent.futures import ThreadPoolExecutor
from threading import Lock


class CommandQueue:
    """Bound active commands and allow a Run's queued foreground work to stop."""

    def __init__(self):
        self._executor = ThreadPoolExecutor(max_workers=8, thread_name_prefix='execution-pod')
        self._lock = Lock()
        self._waiting = {}

    def submit(self, run_id, command, execute, *, background=False):
        if not isinstance(run_id, str) or not run_id:
            raise ValueError('run_id must be a nonempty string')
        with self._lock:
            future = self._executor.submit(execute, command)
            if not background:
                self._waiting.setdefault(run_id, set()).add(future)
        if not background:
            future.add_done_callback(lambda done: self._forget(run_id, done))
        return future

    def _forget(self, run_id, future):
        with self._lock:
            pending = self._waiting.get(run_id)
            if pending is not None:
                pending.discard(future)
                if not pending:
                    del self._waiting[run_id]

    def cancel_waiting(self, run_id):
        with self._lock:
            pending = tuple(self._waiting.get(run_id, ()))
        for future in pending:
            future.cancel()

    def close(self):
        self._executor.shutdown(wait=True, cancel_futures=True)
