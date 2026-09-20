"""Generate credentials once, without printing or replacing existing secrets."""
from pathlib import Path
import os
import secrets

root = Path(__file__).resolve().parent / "secrets"
root.mkdir(mode=0o700, exist_ok=True)
names = ("postgres_password", "runtime_password", "maintenance_database_url", "runtime_database_url", "bootstrap_secret", "signing_key")
if any((root / name).exists() for name in names):
    raise SystemExit("Secret files already exist; preserve them. Rotate credentials explicitly.")
maintenance = secrets.token_hex(32)
runtime = secrets.token_hex(32)
values = {
    "postgres_password": maintenance,
    "runtime_password": runtime,
    "maintenance_database_url": f"postgres://layrs_maintenance:{maintenance}@postgres:5432/layrs",
    "runtime_database_url": f"postgres://layrs_runtime:{runtime}@postgres:5432/layrs",
    "bootstrap_secret": secrets.token_hex(32),
    "signing_key": secrets.token_hex(32),
}
for name, value in values.items():
    # Compose mounts each file directly. Read permission lets UID 10001 consume
    # it; the host directory remains 0700 and protects files from other users.
    fd = os.open(root / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        stream.write(value + "\n")
print("Created six secret files. Restrict directory access and store an encrypted backup.")
