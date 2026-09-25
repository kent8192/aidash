FROM python:3.13-slim-bookworm@sha256:2325bb286ec344af3e5898cc224b5844e2707ac6e26b1632516fd3edc84a5e26
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
COPY requirements.lock /opt/aidash/requirements.lock
RUN pip install --no-cache-dir --require-hashes -r /opt/aidash/requirements.lock
COPY sandbox.py collector.py kernel.py python_cells.py probe.py extract.py install.py /opt/aidash/
WORKDIR /work
USER 10000:10000
ENTRYPOINT ["python", "-I", "/opt/aidash/sandbox.py"]
