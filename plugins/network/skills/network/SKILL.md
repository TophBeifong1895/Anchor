---
name: network
description: Perform HTTP requests and download files when task networking is authorized.
---

Use `network-http_request` to access an HTTP(S) endpoint. It returns the status,
response headers, final URL, text body and whether that body was truncated.
Use `network-http_download` for binary files; choose a relative path in the task
workspace. Existing files are never overwritten.

Business Plugins use their own clients and tools. Academic literature search,
full-text reading and citation analysis belong to the academic-research Plugin.

The Host controls real network access. Network-dependent Plugins can enable this
capability, but an explicit networking denial takes precedence. Changes to mounts
and networking permission take effect from the next Turn.
