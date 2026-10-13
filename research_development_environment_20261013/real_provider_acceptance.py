"""Architect acceptance of private development environments in isolated roots.

Python only orchestrates the Rust Host, pinned Goose and native MCP tools.
Credentials are read as data, passed to the isolated process and redacted from
evidence. Existing deployments, Sessions, experiments and archives are untouched.
"""
import argparse
import base64
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request
import zipfile

REPO = Path(__file__).resolve().parents[1]
FIXTURE = REPO / "rust/anchor-runner-host/tests/fixtures/development_environment"


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def formal_wheel(directory):
    """A local, deterministic source for the existing formal dependency path."""
    prefix = "formalprobe-1.0.dist-info"
    files = {
        "formalprobe/__init__.py": b"VALUE = 'formal-independent'\n",
        prefix + "/METADATA": b"Metadata-Version: 2.1\nName: formalprobe\nVersion: 1.0\n",
        prefix + "/WHEEL": b"Wheel-Version: 1.0\nGenerator: anchor-acceptance\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    }
    records = io.StringIO()
    writer = csv.writer(records, lineterminator="\n")
    for name, content in files.items():
        encoded = base64.urlsafe_b64encode(hashlib.sha256(content).digest()).decode().rstrip("=")
        writer.writerow((name, "sha256=" + encoded, len(content)))
    writer.writerow((prefix + "/RECORD", "", ""))
    files[prefix + "/RECORD"] = records.getvalue().encode()
    path = directory / "formalprobe-1.0-py3-none-any.whl"
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as wheel:
        for name, content in files.items():
            info = zipfile.ZipInfo(name, date_time=(2026, 10, 13, 0, 0, 0))
            info.external_attr = 0o100644 << 16
            wheel.writestr(info, content)
    return path


def json_objects(value):
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except ValueError:
            return
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from json_objects(child)
    elif isinstance(value, list):
        for child in value:
            yield from json_objects(child)


def tool_calls(stream):
    calls = {}
    for line in stream.splitlines():
        if not line.startswith("data: "):
            continue
        event = json.loads(line[6:])
        identity = event.get("toolCallId")
        if event.get("type") == "tool-input-available":
            calls[identity] = {"name": event.get("toolName"), "input": event.get("input")}
        elif event.get("type") in ("tool-output-available", "tool-output-error"):
            calls.setdefault(identity, {}).update(event)
    return list(calls.values())


def configured_environment():
    allowed = {
        "ANCHOR_MODEL_URL", "ANCHOR_MODEL_API_KEY", "ANCHOR_MODEL_NAME",
        "ANCHOR_MODEL_WIRE_API", "ANCHOR_MODEL_CONTEXT_WINDOW",
        "ANCHOR_MODEL_MAX_TOKENS", "ANCHOR_MODEL_DIRECT", "ANCHOR_MODEL_ALIASES",
        "ANCHOR_DEPENDENCIES_PYTHON", "ANCHOR_GOOSE_BINARY",
    }
    result = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TZ": "UTC"}
    for line in (REPO / ".env").read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.removeprefix("export ").split("=", 1)
        if key.strip() in allowed:
            parsed = shlex.split(value, comments=True)
            assert len(parsed) <= 1, "Expected one literal environment value"
            result[key.strip()] = parsed[0] if parsed else ""
    for key in allowed:
        if key in os.environ:
            result[key] = os.environ[key]
    for key in ("ANCHOR_MODEL_URL", "ANCHOR_MODEL_API_KEY", "ANCHOR_MODEL_NAME",
                "ANCHOR_DEPENDENCIES_PYTHON"):
        assert result.get(key), "Missing configured " + key
    return result


class Acceptance:
    def __init__(self, args):
        self.args = args
        self.root = args.root.resolve()
        self.root.mkdir(parents=True, mode=0o700, exist_ok=False)
        self.private = [secrets.token_hex(24)]
        self.process = None
        self.log = None
        self.nonce = secrets.token_hex(8)
        self.plugin = "env-probe-" + self.nonce
        self.session = "development-" + self.nonce
        self.expected = self.nonce + "-changed"
        self.report = {
            "status": "running", "real_provider": True, "production_data_used": False,
            "nonce": self.nonce, "plugin": self.plugin, "checks": {},
            "limitations": [
                "Offline stdlib PEP 660 fixture and a synthetic pure Python formal wheel; no third-party backend or native package claim.",
                "Formal installation keeps its existing contract, without a mandatory successful-test gate.",
                "No deployment or production-data acceptance.",
            ],
        }

    def redact(self, text):
        for value in self.private:
            if value:
                text = text.replace(value, "<redacted>")
        return text

    def save(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, indent=2)
        path.write_text(self.redact(text) + "\n")

    def api(self, method, path, body=None):
        request = urllib.request.Request(
            self.base + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Content-Type": "application/json", "Authorization": "Bearer " + self.private[0]},
        )
        try:
            response = urllib.request.urlopen(request, timeout=20)
        except urllib.error.HTTPError as error:
            raise RuntimeError(
                f"{method} {path}: HTTP {error.code}: {self.redact(error.read().decode())}"
            ) from error
        with response:
            raw = response.read().decode()
            try:
                return response.status, json.loads(raw)
            except ValueError:
                return response.status, raw

    def wait(self, read, done, label, timeout=900):
        deadline = time.monotonic() + timeout
        progress = time.monotonic()
        while True:
            value = read()
            if done(value):
                return value
            assert time.monotonic() < deadline, label + " timed out"
            if self.process is not None:
                assert self.process.poll() is None, "Isolated Host exited"
            if time.monotonic() - progress >= 25:
                print(label + " is running", flush=True)
                progress = time.monotonic()
            time.sleep(0.5)

    def start(self):
        env = configured_environment()
        self.private.append(env["ANCHOR_MODEL_API_KEY"])
        python = Path(env["ANCHOR_DEPENDENCIES_PYTHON"]).resolve()
        goose = Path(env.get("ANCHOR_GOOSE_BINARY", REPO / ".local/rust/bin/goose")).resolve()
        uv = Path(shutil.which("uv") or "").resolve()
        assert uv.is_file(), "uv must be on the invoking PATH"
        binary = self.root / "bin/anchor-runner-host"
        binary.parent.mkdir()
        shutil.copy2(self.args.host.resolve(), binary)
        for name in ("anchor-net-relay", "anchor-sandbox-network"):
            shutil.copy2(REPO / ".local/rust/bin" / name, binary.parent / name)
        sources = self.root / "wheels"
        sources.mkdir()
        wheel = formal_wheel(sources)
        for name in ("state", "workspaces", "library", "catalog", "home"):
            (self.root / name).mkdir()
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        self.base = f"http://127.0.0.1:{port}"
        self.report.update(
            model=env["ANCHOR_MODEL_NAME"], base_url=self.base,
            host_sha256=digest(binary), goose_sha256=digest(goose),
            interpreter=str(python), interpreter_sha256=digest(python), uv_sha256=digest(uv),
            fixture_sha256={p.name: digest(p) for p in FIXTURE.iterdir() if p.is_file()},
            configuration_sha256=digest(REPO / ".env"),
            helper_sha256={p.name: digest(p) for p in binary.parent.iterdir() if p != binary},
            formal_wheel_sha256=digest(wheel),
        )
        env.update({
            "HOME": str(self.root / "home"), "XDG_CONFIG_HOME": str(self.root / "home/config"),
            "XDG_DATA_HOME": str(self.root / "home/data"), "XDG_STATE_HOME": str(self.root / "home/state"),
            "XDG_CACHE_HOME": str(self.root / "home/cache"),
            "ANCHOR_API_KEYS": json.dumps([self.private[0]]),
            "ANCHOR_MODEL_ALIASES": json.dumps({"models.worker": env["ANCHOR_MODEL_NAME"]}),
            "ANCHOR_RUNNER_STATE_ROOT": str(self.root / "state"),
            "ANCHOR_RUNNER_WORKSPACE_ROOT": str(self.root / "workspaces"),
            "ANCHOR_RUNNER_CATALOG_ROOT": str(self.root / "catalog"),
            "ANCHOR_RUNNER_LIBRARY_ROOT": str(self.root / "library"),
            "ANCHOR_RUNNER_LISTEN": f"127.0.0.1:{port}",
            "ANCHOR_RUNNER_ALLOWED_COMMANDS": "sh,cat," + python.name,
            "ANCHOR_GOOSE_BINARY": str(goose), "ANCHOR_GOOSE_BINARY_SHA256": digest(goose),
            "ANCHOR_GOOSE_LOCAL_NETWORK": "1",
            "ANCHOR_GOOSE_RELAY_BINARY": str(binary.parent / "anchor-net-relay"),
            "ANCHOR_SANDBOX_NETWORK_BINARY": str(binary.parent / "anchor-sandbox-network"),
            "ANCHOR_PILOT_PYTHON_ENABLED": "1",
            "ANCHOR_DEPENDENCIES_UV": str(uv), "ANCHOR_DEPENDENCIES_UV_SHA256": digest(uv),
            "ANCHOR_DEPENDENCIES_SOURCES": str(sources),
            "ANCHOR_DEPENDENCIES_BUILD_ROOT": str(self.root / "build"),
            "ANCHOR_NODE_TOOL_DISCLOSURE": "0",
        })
        self.log = (self.root / "host.log").open("wb")
        self.process = subprocess.Popen(
            [str(binary), "serve"], cwd=self.root, env=env,
            stdout=self.log, stderr=self.log, start_new_session=True,
        )
        def ready():
            try:
                return self.api("GET", "/ready")[0]
            except (urllib.error.URLError, RuntimeError):
                return 0
        self.wait(ready, lambda value: value == 200, "Host readiness", 30)
        self.save("identity.json", self.report)
        print("Isolated real Provider Host ready: " + str(self.root), flush=True)

    def conversation(self, session, label, prompt):
        status, accepted = self.api("POST", f"/sessions/{session}/turns", {
            "request_id": label + "-" + self.nonce, "message": prompt,
        })
        assert status == 202, accepted
        # This task does not change mounts or runtime mid-Turn, so no continuation
        # is required. Only the existing install confirmation is accepted here.
        answered = set()
        # Keep the exact admission identity; never infer it from the latest list.
        identity = accepted["turn"]["id"]
        def read():
            turn = self.api("GET", f"/sessions/{session}/turns/{identity}")[1]["turn"]
            questions = self.api("GET", f"/sessions/{session}/turns/{identity}/questions")[1]["questions"]
            for question in questions:
                if question["status"] != "pending" or question["id"] in answered:
                    continue
                required = question["requested_schema"].get("required", [])
                assert self.plugin in question["message"], "Unexpected native authorization target"
                assert required and set(required) <= {"confirm", "replace_existing", "accept_unreadable"}
                assert all(question["requested_schema"]["properties"][key]["type"] == "boolean"
                           for key in required), "Unexpected native form"
                self.save(f"{label}/question-{question['id']}.json", question)
                self.api("POST", f"/sessions/{session}/turns/{identity}/questions/{question['id']}/answer", {
                    "action": "accept", "content": {key: True for key in required},
                })
                answered.add(question["id"])
            return turn
        turn = self.wait(read, lambda item: item["status"] not in ("pending", "running"), label)
        self.save(label + "/turn.json", turn)
        stream = self.api("GET", f"/sessions/{session}/turns/{identity}/events")[1]
        self.save(label + "/events.sse", stream)
        calls = tool_calls(stream)
        self.save(label + "/calls.json", calls)
        assert turn["status"] == "completed", label + ": " + turn["status"]
        return turn, calls

    def develop(self):
        assert self.api("POST", "/sessions", {"id": self.session})[0] == 201
        backend = (FIXTURE / "backend.py").read_text().replace(
            "Requires-Dist: probepkg==1.0; python_version >= '3.8'\\n", ""
        ).replace("Requires-Dist: trialpkg==1.0; extra == 'dev'\\n", "")
        pyproject = (FIXTURE / "pyproject.toml").read_text().replace(
            'dependencies = ["probepkg==1.0; python_version >= \'3.8\'"]', "dependencies = []"
        ).replace('dev = ["trialpkg==1.0"]', "dev = []")
        server = (FIXTURE / "server.py").read_text().replace(
            "for line in sys.stdin:",
            "import formalprobe\nassert formalprobe.VALUE == 'formal-independent'\n"
            "source = pathlib.Path(__file__).parent / 'src/editableprobe/__init__.py'\n"
            "spec = importlib.util.spec_from_file_location('delivered_probe', source)\n"
            "module = importlib.util.module_from_spec(spec)\n"
            "spec.loader.exec_module(module)\n\nfor line in sys.stdin:",
        ).replace(
            '"private_environment_absent": True, "prefix": sys.prefix',
            '"private_environment_absent": True, "prefix": sys.prefix, "source_value": module.VALUE, "formal_value": formalprobe.VALUE',
        )
        manifest = {"name": "Independent environment probe",
                    "dependencies": {"python": ["formalprobe==1.0"]}, "mcpServers": {
            "service": {"command": "${ANCHOR_PLUGIN_VIEW}/bin/python", "args": ["server.py"]},
        }}
        first_source = f"VALUE={self.nonce + '-first'!r}\ndef main(): print(VALUE)\n"
        assets = {
            "one/backend.py": backend, "one/pyproject.toml": pyproject,
            "one/src/editableprobe/__init__.py": first_source,
            "candidate/plugin.json": json.dumps(manifest), "candidate/server.py": server,
        }
        prompt = (
            "在本 Session 完成一个明确授权的离线 Python 开发与 Plugin 安装验收。"
            "先用 anchor_run 创建 one/src/editableprobe 和 candidate/src/editableprobe，"
            "再用 anchor_edit 的 content 写入下面给出的精确文件。"
            "调用 development_prepare(project='one') 创建空私有环境；"
            "用 anchor_run(project='one') 执行 pip install --no-index --no-build-isolation -e '.[dev]'。"
            "实际调用 editable-probe，确认输出 " + self.nonce + "-first。"
            "然后用 anchor_edit 的 old_string/new_string 精确替换 VALUE="
            + repr(self.nonce + "-first") + " 为 VALUE=" + repr(self.expected) + "，保留 main。"
            "再次运行 editable-probe、python -m pip check、python3 和裸 pip，"
            "检查 sys.prefix 与 VIRTUAL_ENV 都是 /development/env。"
            "再默认 development_prepare 一次，确认 created=false 且 editable-probe 仍输出新值。"
            "用普通工作区命令把修改后的源码复制到 candidate/src/editableprobe/__init__.py，"
            "候选只包含给出的 server、manifest 和这份源码，不带私有环境。"
            "调用 plugin_prepare(directory='candidate')，然后 plugin_test(directory='candidate',"
            "server='service',tool='probe',arguments={'value':'candidate-" + self.nonce + "'})。"
            "核对实际业务返回 private_environment_absent=true 和 source_value=" + self.expected + "。"
            "正式依赖 formalprobe==1.0 由 Host 的隔离本地 wheel 来源准备，不装入项目私有环境。"
            "最后 plugin_install(directory='candidate',id='" + self.plugin + "')，等待原生确认并核查安装成功。"
            "本轮不挂载 Plugin、不申请联网、不创建 Graph；最后简述实际工具结果。"
            "精确文件数据：\n" + json.dumps(assets, ensure_ascii=False)
        )
        turn, calls = self.conversation(self.session, "development", prompt)
        successful = [call for call in calls if call.get("type") == "tool-output-available"
                      and not any(item.get("isError") is True for item in json_objects(call.get("output")))]
        assert all(any(call.get("name") == name for call in successful)
                   for name in ("anchor_edit", "development_prepare", "anchor_run",
                                "plugin_prepare", "plugin_test", "plugin_install")), "Missing successful native step"
        prepares = [item for call in successful if call["name"] == "development_prepare"
                    for item in json_objects(call.get("output")) if item.get("environment_kind") == "private"]
        assert any(item.get("created") is True and item.get("pip_available") is True for item in prepares)
        assert any(item.get("created") is False for item in prepares), "Default ensure did not preserve an existing env"
        runs = [item for call in successful if call["name"] == "anchor_run"
                for item in json_objects(call.get("output")) if item.get("exit_code") == 0]
        assert any(self.nonce + "-first" in item.get("stdout", "") for item in runs)
        assert any(self.expected in item.get("stdout", "") for item in runs), "Source edit was not retested"
        candidate = [item for call in successful if call["name"] == "plugin_test"
                     for item in json_objects(call.get("output"))
                     if item.get("private_environment_absent") is True and item.get("source_value") == self.expected]
        assert candidate, "Independent candidate business result missing"
        scope = turn["goose"]["scope"]
        binding = json.loads((self.root / "state/platform/pilot-resources" / (scope + ".json")).read_text())
        self.workspace = Path(binding["workspace"])
        selections = list((self.root / "state/platform/pilot").rglob("selection.json"))
        assert len(selections) == 1, "Expected exactly one private project selection"
        self.selection_path = selections[0]
        fact = json.loads(self.selection_path.read_text())
        self.save("development/selection.json", fact)
        assert fact["interpreter_digest"] == self.report["interpreter_sha256"]
        assert fact["state"] == "private"
        self.report["checks"].update(
            private_pip=True, editable=True, source_change_retested=True,
            ensure_preserved_existing_environment=True, candidate_business_tested_independently=True,
            native_install_confirmed=True,
        )

    def formal_run(self, label):
        graph = "formal-" + self.nonce
        value = label + "-" + self.nonce
        if label == "before-delete":
            definition = {
                "entry": "worker",
                "agents": {"worker": {
                    "model": "models.worker", "writes": ["proof.json"],
                    "instructions": "Call the installed " + self.plugin + "-service_probe tool with "
                    "the value from the task. Save its actual returned JSON to proof.json. Check "
                    "private_environment_absent=true and source_value=" + self.expected
                    + ". Finish with the native final_result tool.",
                }},
                "nodes": [{"id": "worker", "agent": "worker", "plugins": [self.plugin]}],
                "edges": [],
            }
            assert self.api("POST", "/graphs", {"name": graph, "definition": definition})[0] == 201
        status, started = self.api("POST", "/trigger", {
            "graph": graph, "objective": "Call the probe with value=" + value + " and save proof.json.",
            "input": {"value": value},
        })
        assert status == 202, started
        run = started["run"]
        detail = self.wait(lambda: self.api("GET", f"/runs/{run}")[1],
                           lambda item: not item["active"], "Formal " + label)
        self.save(label + "/run.json", detail)
        assert detail["state"]["status"] == "completed", detail["state"].get("error")
        artifact = self.api("GET", f"/runs/{run}/files/worker/proof.json")[1]
        self.save(label + "/artifact.json", artifact)
        observation = json.loads(artifact["text"])
        assert observation["private_environment_absent"] is True
        assert observation["source_value"] == self.expected
        assert observation["value"] == value
        assert observation["formal_value"] == "formal-independent"
        # Verify against native tool output as well as the model-written artifact.
        records = list((self.root / "state/goose-acp").glob("*.evidence.json"))
        native = [record for path in records
                  if (record := json.loads(path.read_text())).get("key", {}).get("run_id") == run]
        results = [item for record in native for call in record.get("tool_calls", [])
                   if call.get("tool") == self.plugin + "-service_probe"
                   and call.get("result", {}).get("ok") is True
                   and call.get("arguments", {}).get("value") == value
                   for item in json_objects(call.get("result"))
                   if item.get("private_environment_absent") is True
                   and item.get("source_value") == self.expected and item.get("value") == value
                   and item.get("formal_value") == "formal-independent"]
        assert results, "Missing actual formal MCP result"
        assert observation in results, "Artifact disagrees with this Run's actual MCP result"
        self.save(label + "/native-evidence.json", native)
        self.report["checks"][label] = {"run": run, "actual_results": results, "artifact": observation}

    def finish(self):
        self.formal_run("before-delete")
        status, result = self.api("DELETE", f"/sessions/{self.session}")
        assert status == 200, result
        self.save("session-deletion.json", result)
        assert not self.selection_path.exists()
        assert not self.workspace.exists()
        assert (self.root / "library/plugins" / self.plugin / "plugin.json").is_file()
        fresh = "after-cleanup-" + self.nonce
        assert self.api("POST", "/sessions", {"id": fresh})[0] == 201
        status, mounted = self.api("POST", f"/sessions/{fresh}/plugins/{self.plugin}", {"on": True})
        assert status == 200, mounted
        self.save("after-cleanup-inspect/mount.json", mounted)
        value = "fresh-session-" + self.nonce
        _, calls = self.conversation(
            fresh, "after-cleanup-inspect", "先用 platform_status 核查平台，然后实际调用已启用的 "
            + self.plugin + "-service_probe，参数 value=" + value
            + "。核对 private_environment_absent=true、source_value=" + self.expected
            + " 和 formal_value=formal-independent，简述实际返回结果。",
        )
        results = [item for call in calls
                   if call.get("name") == self.plugin + "-service_probe"
                   and call.get("type") == "tool-output-available"
                   and call.get("input", {}).get("value") == value
                   for item in json_objects(call.get("output"))
                   if item.get("value") == value and item.get("source_value") == self.expected
                   and item.get("private_environment_absent") is True
                   and item.get("formal_value") == "formal-independent"]
        assert results, "Fresh Session did not actually call the installed Plugin"
        self.report["checks"]["fresh_session_actual_call"] = results
        self.formal_run("after-delete")
        status, cleaned = self.api("DELETE", f"/sessions/{fresh}")
        assert status == 200, cleaned
        self.save("after-cleanup-inspect/deletion.json", cleaned)
        assert digest(self.report["interpreter"]) == self.report["interpreter_sha256"]
        assert digest(REPO / ".env") == self.report["configuration_sha256"]
        self.report["checks"].update(
            private_selection_and_workspace_removed=True,
            formal_plugin_survived_session_delete=True, shared_interpreter_unchanged=True,
            configuration_unchanged=True,
        )
        self.report["status"] = "passed"

    def stop(self):
        if self.process is not None and self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=10)
        if self.process is not None:
            self.report["host_exit_code"] = self.process.returncode
            with socket.socket() as probe:
                probe.settimeout(1)
                address, port = self.base.removeprefix("http://").rsplit(":", 1)
                self.report["listener_closed"] = probe.connect_ex((address, int(port))) != 0
        if self.log is not None:
            self.log.close()
            path = self.root / "host.log"
            path.write_text(self.redact(path.read_text(errors="replace")))
        self.save("evidence.json", self.report)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", type=Path, default=REPO / "rust/target/debug/anchor-runner-host")
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    review = Acceptance(args)
    try:
        review.start()
        review.develop()
        review.finish()
        print("Real Provider acceptance passed: " + str(review.root / "evidence.json"), flush=True)
    except Exception as error:
        review.report.update(status="failed", error=review.redact(str(error)))
        print("Acceptance failed: " + review.redact(str(error)), flush=True)
        raise
    finally:
        review.stop()


if __name__ == "__main__":
    main()
