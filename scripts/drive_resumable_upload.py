#!/usr/bin/env python3
"""Resumable upload a file to a Google shared-drive folder using the gws service account.

Usage: drive_resumable_upload.py <file> <folder_id> [name]
"""
import os, sys, json, warnings
warnings.filterwarnings("ignore")
from google.oauth2 import service_account
from google.auth.transport.requests import AuthorizedSession

SA = os.environ.get("GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE", "/Users/zack/.config/gws/service-account.json")
path, folder_id = sys.argv[1], sys.argv[2]
name = sys.argv[3] if len(sys.argv) > 3 else os.path.basename(path)
size = os.path.getsize(path)

creds = service_account.Credentials.from_service_account_file(
    SA, scopes=["https://www.googleapis.com/auth/drive"])
sess = AuthorizedSession(creds)

# 1) start resumable session
meta = {"name": name, "parents": [folder_id]}
r = sess.post(
    "https://www.googleapis.com/upload/drive/v3/files",
    params={"uploadType": "resumable", "supportsAllDrives": "true"},
    headers={"Content-Type": "application/json; charset=UTF-8",
             "X-Upload-Content-Type": "application/zip",
             "X-Upload-Content-Length": str(size)},
    data=json.dumps(meta))
r.raise_for_status()
session_uri = r.headers["Location"]
print(f"session started, uploading {size/1e9:.2f} GB ...", flush=True)

# 2) upload in chunks (resumable)
CHUNK = 32 * 1024 * 1024  # 32 MiB, must be multiple of 256 KiB
with open(path, "rb") as f:
    sent = 0
    while sent < size:
        chunk = f.read(CHUNK)
        end = sent + len(chunk) - 1
        hdr = {"Content-Length": str(len(chunk)),
               "Content-Range": f"bytes {sent}-{end}/{size}"}
        resp = sess.put(session_uri, headers=hdr, data=chunk)
        if resp.status_code in (200, 201):
            info = resp.json()
            print(json.dumps({"ok": True, "id": info["id"], "name": info["name"],
                              "link": f"https://drive.google.com/file/d/{info['id']}/view"}))
            sys.exit(0)
        elif resp.status_code == 308:  # incomplete, continue
            sent = end + 1
            print(f"  {sent/1e9:.2f}/{size/1e9:.2f} GB", flush=True)
        else:
            print(f"ERROR {resp.status_code}: {resp.text}", file=sys.stderr)
            sys.exit(1)
