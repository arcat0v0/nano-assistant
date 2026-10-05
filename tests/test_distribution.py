import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import threading
import time
import unittest
from email.parser import BytesParser
from email.policy import default
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-linux-gnu", "x86_64-linux-musl", "aarch64-linux-musl")
ARTIFACT = "na-x86_64-linux-musl.tar.gz"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def archive(version="0.3.2", executable=True, stderr=False):
    redirect = " >&2" if stderr else ""
    payload = f"#!/bin/sh\nprintf 'na {version}\\n'{redirect}\n".encode()
    if not executable:
        payload = b"invalid executable\n"
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as tar:
        entry = tarfile.TarInfo("na")
        entry.size = len(payload)
        entry.mode = 0o755
        tar.addfile(entry, io.BytesIO(payload))
    return output.getvalue()


class DistributionServer:
    def __init__(self):
        self.files = {}
        self.releases = {"github": [], "gitee": []}
        self.requests = []
        self.failures = {}
        self.delays = {}
        self.country = b"loc=CN\n"
        self.browser_aliases = False
        self.missing_gitee_release_is_null = False
        state = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.handle_request()

            def do_POST(self):
                self.handle_request()

            def do_PATCH(self):
                self.handle_request()

            def handle_request(self):
                parsed = urlsplit(self.path)
                path = unquote(parsed.path)
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                state.requests.append((self.command, path, dict(self.headers), body))
                time.sleep(state.delays.get(path, 0))
                failure = state.failures.get((self.command, path))
                if failure:
                    self.respond(failure, b"controlled failure")
                    return
                if path == "/country":
                    self.respond(200, state.country)
                    return
                if path in state.files:
                    self.respond(200, state.files[path])
                    return
                parts = path.strip("/").split("/")
                if (
                    len(parts) < 5
                    or parts[0] not in state.releases
                    or parts[1:4] != ["repos", "acme", "na"]
                ):
                    self.respond(404, b"missing")
                    return
                source = parts[0]
                releases = state.releases[source]
                suffix = parts[4:]
                if suffix == ["releases"]:
                    if self.command == "GET":
                        query = parse_qs(parsed.query)
                        page = int(query.get("page", ["1"])[0])
                        self.respond(200, releases[(page - 1) * 100 : page * 100])
                    else:
                        values = json.loads(body)
                        release = state.release(
                            source,
                            values["tag_name"],
                            **{
                                key: values[key]
                                for key in ("name", "body", "draft", "prerelease")
                                if key in values
                            },
                        )
                        releases.append(release)
                        self.respond(201, release)
                    return
                if suffix[:2] == ["releases", "tags"]:
                    release = next(
                        (r for r in releases if r["tag_name"] == suffix[2]), None
                    )
                    if (
                        release is None
                        and source == "gitee"
                        and state.missing_gitee_release_is_null
                    ):
                        self.respond(200, None)
                        return
                    self.respond(200 if release else 404, release or {})
                    return
                release = next((r for r in releases if str(r["id"]) == suffix[1]), None)
                if not release:
                    self.respond(404, {})
                    return
                if len(suffix) == 2:
                    if self.command == "PATCH":
                        release.update(json.loads(body))
                    self.respond(200, release)
                    return
                if suffix[2] in ("assets", "attach_files"):
                    if self.command == "GET":
                        self.respond(200, release["assets"])
                        return
                    if source == "github":
                        name = parse_qs(parsed.query)["name"][0]
                        data = body
                    else:
                        message = BytesParser(policy=default).parsebytes(
                            f"Content-Type: {self.headers['Content-Type']}\r\n\r\n".encode()
                            + body
                        )
                        part = next(
                            p
                            for p in message.iter_parts()
                            if p.get_param("name", header="Content-Disposition")
                            == "file"
                        )
                        name = part.get_filename()
                        data = part.get_payload(decode=True)
                    asset = state.asset(source, release["tag_name"], name, data)
                    release["assets"].append(asset)
                    self.respond(201, asset)
                    return
                self.respond(404, {})

            def respond(self, status, data):
                if not isinstance(data, bytes):
                    data = json.dumps(data).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                except BrokenPipeError:
                    pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def asset(self, source, tag, name, data):
        path = f"/{source}/releases/download/{tag}/{name}"
        self.files[path] = data
        browser_path = path
        if self.browser_aliases:
            browser_path = f"/cdn/{source}/{tag}/{name}"
            self.files[browser_path] = data
        return {
            "id": len(self.files),
            "name": name,
            "size": len(data),
            "browser_download_url": self.base + browser_path,
        }

    def release(self, source, tag, **values):
        return {
            "id": len(self.releases[source]) + 1,
            "tag_name": tag,
            "name": tag,
            "body": "notes",
            "draft": False,
            "prerelease": False,
            "assets": [],
            "upload_url": self.base
            + f"/{source}/repos/acme/na/releases/{len(self.releases[source]) + 1}/assets{{?name,label}}",
            **values,
        }

    def ready_release(self, source, version="0.3.2", ready=True, stderr=False):
        tag = "v" + version
        release = self.release(source, tag)
        self.releases[source].append(release)
        files = {}
        for target in TARGETS:
            name = f"na-{target}.tar.gz"
            files[name] = archive(version, stderr=stderr)
            files[name + ".sha256"] = f"{digest(files[name])}  {name}\n".encode()
        files["install.sh"] = b"installer"
        if ready:
            files["release-manifest.json"] = json.dumps(
                {
                    "schema": 1,
                    "tag": tag,
                    "commit": "a" * 40,
                    "assets": {name: digest(data) for name, data in files.items()},
                }
            ).encode()
        for name, data in files.items():
            release["assets"].append(self.asset(source, tag, name, data))
        return release


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.install_dir = self.home / "bin"
        self.install_dir.mkdir()
        self.binary = self.install_dir / "na"
        self.binary.write_text("old installation")
        self.config = self.home / "config/nano-assistant/config.toml"
        self.config.parent.mkdir(parents=True)
        self.config.write_text("existing configuration")
        self.server = self.enterContext(DistributionServer())

    def install(self, **overrides):
        env = {
            **os.environ,
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.home / "config"),
            "NA_INSTALL_DIR": str(self.install_dir),
            "NA_SOURCE": "auto",
            "NA_VERSION": "latest",
            "NA_GITHUB_API_URL": self.server.base + "/github/repos/acme/na",
            "NA_GITEE_API_URL": self.server.base + "/gitee/repos/acme/na",
            "NA_GITHUB_RELEASES_URL": self.server.base + "/github/releases",
            "NA_GITEE_RELEASES_URL": self.server.base + "/gitee/releases",
            "NA_COUNTRY_URL": self.server.base + "/country",
            "NA_PROBE_TIMEOUT": "1",
            "NA_CONNECT_TIMEOUT": "1",
            "NA_DOWNLOAD_TIMEOUT": "3",
            "HTTPS_PROXY": self.server.base,
            "HTTP_PROXY": self.server.base,
            "https_proxy": self.server.base,
            "http_proxy": self.server.base,
            "ALL_PROXY": self.server.base,
            "all_proxy": self.server.base,
            "NO_PROXY": "127.0.0.1",
            "no_proxy": "127.0.0.1",
        }
        env.pop("NA_BASE_URL", None)
        env.update(overrides)
        return subprocess.run(
            ["bash", str(ROOT / "install.sh")],
            env=env,
            capture_output=True,
            text=True,
            timeout=20,
        )

    def test_mainland_uses_gitee_and_preserves_config(self):
        self.server.ready_release("gitee")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("gitee", result.stdout)
        self.assertIn("0.3.2", self.binary.read_text())
        self.assertEqual(self.config.read_text(), "existing configuration")
        self.assertFalse(
            any(path.startswith("/github") for _, path, _, _ in self.server.requests)
        )

    def test_version_output_on_stderr_is_validated(self):
        self.server.ready_release("gitee", stderr=True)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0.3.2", self.binary.read_text())
        self.assertEqual(self.config.read_text(), "existing configuration")

    def test_overseas_prefers_github(self):
        self.server.country = b"loc=SG\n"
        self.server.ready_release("github")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("github", result.stdout)

    def test_unknown_country_uses_available_source(self):
        self.server.failures[("GET", "/country")] = 503
        self.server.failures[("GET", "/github/repos/acme/na/releases")] = 503
        self.server.ready_release("gitee")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("unknown", result.stdout)

    def test_latest_fallback_stays_on_resolved_version(self):
        self.server.ready_release("gitee")
        self.server.ready_release("github", "0.3.3")
        self.server.ready_release("github")
        self.server.failures[
            ("GET", f"/gitee/releases/download/v0.3.2/{ARTIFACT}.sha256")
        ] = 503
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("0.3.2", self.binary.read_text())
        self.assertFalse(
            any(
                path.startswith("/github/repos")
                for _, path, _, _ in self.server.requests
            )
        )

    def test_latest_selects_semver_and_skips_incomplete_and_preview(self):
        self.server.ready_release("gitee", "0.3.9")
        self.server.ready_release("gitee", "0.3.10")
        self.server.ready_release("gitee", "0.3.11", ready=False)
        preview = self.server.ready_release("gitee", "0.4.0")
        preview["prerelease"] = True
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("0.3.10", self.binary.read_text())

    def test_explicit_source_skips_detection_and_never_falls_back(self):
        self.server.ready_release("github")
        result = self.install(NA_SOURCE="gitee", NA_VERSION="0.3.2")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.binary.read_text(), "old installation")
        self.assertFalse(
            any(
                path == "/country" or path.startswith("/github")
                for _, path, _, _ in self.server.requests
            )
        )

    def test_checksum_mismatch_does_not_replace_existing_binary(self):
        self.server.ready_release("gitee")
        self.server.ready_release("github")
        self.server.files[f"/gitee/releases/download/v0.3.2/{ARTIFACT}"] = b"corrupted"
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.binary.read_text(), "old installation")
        self.assertFalse(
            any(path.startswith("/github") for _, path, _, _ in self.server.requests)
        )

    def test_invalid_binary_does_not_replace_existing_binary(self):
        data = archive(executable=False)
        self.server.files[f"/custom/download/v0.3.2/{ARTIFACT}"] = data
        self.server.files[f"/custom/download/v0.3.2/{ARTIFACT}.sha256"] = (
            f"{digest(data)}  {ARTIFACT}\n".encode()
        )
        result = self.install(
            NA_BASE_URL=self.server.base + "/custom", NA_VERSION="0.3.2"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.binary.read_text(), "old installation")

    def test_custom_base_keeps_legacy_latest_contract(self):
        data = archive()
        self.server.files[f"/custom/latest/download/{ARTIFACT}"] = data
        self.server.files[f"/custom/latest/download/{ARTIFACT}.sha256"] = (
            f"{digest(data)}  {ARTIFACT}\n".encode()
        )
        result = self.install(
            NA_BASE_URL=self.server.base + "/custom", NA_SOURCE="invalid"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(
            any(path == "/country" for _, path, _, _ in self.server.requests)
        )

    def test_invalid_source_and_version_fail_before_network(self):
        for values in ({"NA_SOURCE": "invalid"}, {"NA_VERSION": "../../bad"}):
            with self.subTest(values=values):
                result = self.install(**values)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.binary.read_text(), "old installation")
        self.assertEqual(self.server.requests, [])

    def test_country_timeout_is_bounded_and_does_not_prevent_install(self):
        self.server.delays["/country"] = 2
        self.server.ready_release("github")
        started = time.monotonic()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("unknown", result.stdout)
        self.assertLess(time.monotonic() - started, 5)

    def test_arm64_and_unsupported_architecture(self):
        fake_bin = self.home / "commands"
        fake_bin.mkdir()
        uname = fake_bin / "uname"
        uname.write_text(
            '#!/bin/sh\nif [ "$1" = -s ]; then echo Linux; else echo "$TEST_ARCH"; fi\n'
        )
        uname.chmod(0o755)
        self.server.ready_release("gitee")
        path = str(fake_bin) + os.pathsep + os.environ["PATH"]
        result = self.install(PATH=path, TEST_ARCH="aarch64")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(
            any(
                "na-aarch64-linux-musl.tar.gz" in url
                for _, url, _, _ in self.server.requests
            )
        )
        self.server.requests.clear()
        result = self.install(PATH=path, TEST_ARCH="riscv64")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.server.requests, [])

    def test_wget_download_path(self):
        commands = self.home / "commands"
        commands.mkdir()
        for name in (
            "bash",
            "uname",
            "jq",
            "sha256sum",
            "tar",
            "gzip",
            "install",
            "mktemp",
            "timeout",
            "wget",
            "rm",
            "sed",
            "head",
            "awk",
            "mkdir",
            "mv",
            "grep",
        ):
            executable = shutil.which(name)
            self.assertIsNotNone(executable, name)
            (commands / name).symlink_to(executable)
        self.server.ready_release("gitee")
        result = self.install(PATH=str(commands))
        self.assertEqual(result.returncode, 0, result.stderr)

    def bootstrap(self, **overrides):
        block = re.search(
            r"```bash\n(.*?)\n```", (ROOT / "README.md").read_text(), re.DOTALL
        ).group(1)
        temp = self.home / "temporary"
        temp.mkdir()
        env = {
            **os.environ,
            "HOME": str(self.home),
            "TMPDIR": str(temp),
            "NA_GITHUB_INSTALL_URL": self.server.base + "/bootstrap/github",
            "NA_GITEE_INSTALL_URL": self.server.base + "/bootstrap/gitee",
            "HTTPS_PROXY": self.server.base,
            "https_proxy": self.server.base,
            "HTTP_PROXY": self.server.base,
            "http_proxy": self.server.base,
            "ALL_PROXY": self.server.base,
            "all_proxy": self.server.base,
            "NO_PROXY": "127.0.0.1",
            "no_proxy": "127.0.0.1",
            **overrides,
        }
        result = subprocess.run(
            ["bash", "-c", block], env=env, capture_output=True, text=True, timeout=20
        )
        self.assertEqual(list(temp.iterdir()), [])
        return result

    def test_bootstrap_falls_back_and_only_runs_complete_script(self):
        self.server.failures[("GET", "/bootstrap/github")] = 503
        self.server.files["/bootstrap/gitee"] = (
            b'printf bootstrap-ok > "$HOME/started"\n'
        )
        result = self.bootstrap()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.home / "started").read_text(), "bootstrap-ok")

    def test_bootstrap_does_not_execute_on_download_failure(self):
        result = self.bootstrap()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / "started").exists())

    def test_wrong_binary_version_preserves_existing_installation(self):
        data = archive("0.3.1")
        self.server.files[f"/custom/download/v0.3.2/{ARTIFACT}"] = data
        self.server.files[f"/custom/download/v0.3.2/{ARTIFACT}.sha256"] = (
            f"{digest(data)}  {ARTIFACT}\n".encode()
        )
        result = self.install(
            NA_BASE_URL=self.server.base + "/custom", NA_VERSION="0.3.2"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.binary.read_text(), "old installation")


class ReleaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location(
            "distribution_release", ROOT / "scripts/release.py"
        )
        cls.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.module)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.dist = Path(self.temp.name)
        for target in TARGETS:
            name = f"na-{target}.tar.gz"
            data = archive()
            (self.dist / name).write_bytes(data)
            (self.dist / (name + ".sha256")).write_text(f"{digest(data)}  {name}\n")
        self.server = self.enterContext(DistributionServer())

    def prepare(self):
        return self.module.prepare_bundle(
            self.dist, "v0.3.2", "a" * 40, ROOT / "install.sh"
        )

    def publisher(self, source):
        return self.module.ReleasePublisher(
            source,
            "acme/na",
            "controlled-test-token",
            self.server.base + f"/{source}/repos/acme/na",
            self.server.base + f"/{source}/releases",
        )

    def test_prepare_checks_artifacts_and_is_deterministic(self):
        first = self.prepare()
        second = self.prepare()
        self.assertEqual(first, second)
        self.assertEqual(len(first["assets"]), 7)
        (self.dist / ARTIFACT).write_bytes(b"corrupted")
        with self.assertRaises(self.module.ReleaseError):
            self.prepare()

    def test_gitee_creates_release_when_missing_tag_returns_null(self):
        self.prepare()
        self.server.missing_gitee_release_is_null = True
        self.publisher("gitee").publish(self.dist, "v0.3.2", "notes")
        self.assertEqual(len(self.server.releases["gitee"]), 1)
        self.assertEqual(len(self.server.releases["gitee"][0]["assets"]), 8)
        self.assertIn(
            "/gitee/releases/download/v0.3.2/release-manifest.json", self.server.files
        )

    def test_both_platforms_receive_same_bundle_and_retries_are_idempotent(self):
        self.prepare()
        for source in ("github", "gitee"):
            publisher = self.publisher(source)
            publisher.publish(self.dist, "v0.3.2", "notes")
            posts = sum(method == "POST" for method, _, _, _ in self.server.requests)
            publisher.publish(self.dist, "v0.3.2", "notes")
            self.assertEqual(
                posts, sum(method == "POST" for method, _, _, _ in self.server.requests)
            )
            self.assertEqual(len(self.server.releases[source]), 1)
            self.assertFalse(self.server.releases[source][0]["draft"])
        for file in self.dist.iterdir():
            self.assertEqual(
                self.server.files[f"/github/releases/download/v0.3.2/{file.name}"],
                self.server.files[f"/gitee/releases/download/v0.3.2/{file.name}"],
            )
        for method, path, headers, _ in self.server.requests:
            if method == "GET" and "/releases/download/" in path:
                self.assertNotIn("Authorization", headers)

    def test_failed_upload_can_resume_without_publishing_ready_marker_early(self):
        self.prepare()
        path = "/gitee/repos/acme/na/releases/1/attach_files"
        self.server.failures[("POST", path)] = 503
        publisher = self.publisher("gitee")
        with self.assertRaises(self.module.ReleaseError):
            publisher.publish(self.dist, "v0.3.2", "notes")
        self.assertNotIn(
            "/gitee/releases/download/v0.3.2/release-manifest.json", self.server.files
        )
        del self.server.failures[("POST", path)]
        publisher.publish(self.dist, "v0.3.2", "notes")
        self.assertEqual(len(self.server.releases["gitee"]), 1)

    def test_existing_conflicting_asset_is_rejected(self):
        self.prepare()
        publisher = self.publisher("gitee")
        publisher.publish(self.dist, "v0.3.2", "notes")
        path = f"/gitee/releases/download/v0.3.2/{ARTIFACT}"
        self.server.files[path] = b"different published content"
        with self.assertRaises(self.module.ReleaseError):
            publisher.publish(self.dist, "v0.3.2", "notes")
        self.assertEqual(self.server.files[path], b"different published content")

    def test_malformed_manifest_prevents_any_publish_request(self):
        self.prepare()
        (self.dist / "release-manifest.json").write_text('{"tag":"v9.9.9"}')
        with self.assertRaises(self.module.ReleaseError):
            self.publisher("github").publish(self.dist, "v0.3.2", "notes")
        self.assertEqual(self.server.requests, [])

    def test_recovery_reuses_published_bytes_instead_of_rebuilding(self):
        self.prepare()
        publisher = self.publisher("github")
        publisher.publish(self.dist, "v0.3.2", "original notes")
        recovered = self.dist / "recovered"
        publisher.recover(recovered, "v0.3.2", "a" * 40)
        for name in (*self.module.ASSETS, self.module.MANIFEST):
            self.assertEqual(
                (recovered / name).read_bytes(), (self.dist / name).read_bytes()
            )
        self.assertEqual((recovered / "release-notes.md").read_text(), "original notes")

    def test_recovery_can_add_manifest_to_legacy_complete_release(self):
        self.server.ready_release("github", ready=False)
        self.publisher("github").recover(self.dist / "recovered", "v0.3.2", "a" * 40)
        manifest = self.module.load_bundle(self.dist / "recovered", "v0.3.2")
        self.assertEqual(manifest["commit"], "a" * 40)

    def test_recovery_rejects_mismatched_commit(self):
        self.prepare()
        publisher = self.publisher("github")
        publisher.publish(self.dist, "v0.3.2", "notes")
        with self.assertRaises(self.module.ReleaseError):
            publisher.recover(self.dist / "recovered", "v0.3.2", "b" * 40)

    def test_credentials_are_not_sent_to_untrusted_upload_origin(self):
        publisher = self.publisher("github")
        with self.assertRaises(self.module.ReleaseError):
            publisher.api("POST", "", b"data", url="https://untrusted.invalid/upload")
        self.assertEqual(self.server.requests, [])

    def test_ready_marker_requires_the_installers_stable_download_route(self):
        self.prepare()
        self.server.browser_aliases = True
        self.server.failures[("GET", f"/gitee/releases/download/v0.3.2/{ARTIFACT}")] = (
            503
        )
        with self.assertRaises(self.module.ReleaseError):
            self.publisher("gitee").publish(self.dist, "v0.3.2", "notes")
        self.assertNotIn(
            "/gitee/releases/download/v0.3.2/release-manifest.json", self.server.files
        )


class MirrorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        ReleaseTests.setUpClass()
        cls.module = ReleaseTests.module

    def test_tag_and_branch_sync_is_repeatable_and_conflicts_are_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            remote = root / "remote.git"

            def git(*args, cwd=source):
                return subprocess.run(
                    [
                        "git",
                        "-c",
                        "commit.gpgsign=false",
                        "-c",
                        "tag.gpgsign=false",
                        *args,
                    ],
                    cwd=cwd,
                    check=True,
                    capture_output=True,
                    text=True,
                    timeout=10,
                ).stdout.strip()

            git("init", "--bare", str(remote), cwd=root)
            git("init", "-b", "main", str(source), cwd=root)
            git("config", "user.name", "Distribution Test")
            git("config", "user.email", "test@example.invalid")
            (source / "file").write_text("first")
            git("add", "file")
            git("commit", "-m", "first")
            first = git("rev-parse", "HEAD")
            git("tag", "v0.3.2")
            git("update-ref", "refs/remotes/origin/main", first)
            for _ in range(2):
                self.module.sync_repository("v0.3.2", remote.as_uri(), cwd=source)
            self.assertEqual(git("rev-parse", "refs/heads/main", cwd=remote), first)
            (source / "file").write_text("second")
            git("commit", "-am", "second")
            second = git("rev-parse", "HEAD")
            git("update-ref", "refs/tags/v0.3.2", second)
            with self.assertRaises(self.module.ReleaseError):
                self.module.sync_repository("v0.3.2", remote.as_uri(), cwd=source)
            self.assertEqual(git("rev-parse", "refs/tags/v0.3.2", cwd=remote), first)


if __name__ == "__main__":
    unittest.main()
