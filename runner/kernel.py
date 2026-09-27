"""The kernel owns user memory only. The trusted collector sends bounded cells."""
from ipykernel.kernelapp import IPKernelApp
import site

# This is untrusted Agent-owned code, subject to the same sandbox and quotas as
# every cell. It is intentionally absent from the trusted collector's sys.path.
site.addsitedir('/work/.aidash-python/site-packages')

IPKernelApp.launch_instance(["--IPKernelApp.connection_file=/request/kernel.json"])
