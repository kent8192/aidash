"""The kernel owns user memory only. The trusted collector sends bounded cells."""
from ipykernel.kernelapp import IPKernelApp
import os
import site
import sys

# This is untrusted Agent-owned code, subject to the same sandbox and quotas as
# every cell. It is intentionally absent from the trusted collector's sys.path.
if os.environ.get('AIDASH_JOURNAL') == '1':
    sys.path.insert(0, '/opt/aidash')
    from journal_access import journal
    app = IPKernelApp.instance()
    app.initialize(["--IPKernelApp.connection_file=/request/kernel.json"])
    app.shell.user_ns['journal'] = journal
    app.start()
else:
    site.addsitedir('/work/.aidash-python/site-packages')
    IPKernelApp.launch_instance(["--IPKernelApp.connection_file=/request/kernel.json"])
