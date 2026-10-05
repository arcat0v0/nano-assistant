import argparse
from contextlib import ExitStack
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import uuid
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

TARGETS = ("x86_64-linux-gnu", "x86_64-linux-musl", "aarch64-linux-musl")
ASSETS = tuple(
    name
    for target in TARGETS
    for name in (f"na-{target}.tar.gz", f"na-{target}.tar.gz.sha256")
) + ("install.sh",)
MANIFEST = "release-manifest.json"
TAG_PATTERN = r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"


class ReleaseError(Exception):
    pass


class ApiError(ReleaseError):
    def __init__(self, status: int):
        self.status = status
        super().__init__(f"release API request failed (HTTP {status})")


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class PublicRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        validate_url(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def validate_tag(tag: str) -> None:
    if not re.fullmatch(TAG_PATTERN, tag):
        raise ReleaseError("release tag must be vMAJOR.MINOR.PATCH")


def file_digest(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def validate_url(url: str) -> None:
    parsed = urlsplit(url)
    if (
        not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.fragment
        or (
            parsed.scheme != "https"
            and not (
                parsed.scheme == "http"
                and parsed.hostname in ("127.0.0.1", "localhost", "::1")
            )
        )
    ):
        raise ReleaseError("release URLs must use HTTPS or a loopback test server")


def prepare_bundle(dist: Path, tag: str, commit: str, installer: Path) -> dict:
    validate_tag(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ReleaseError("release commit must be a full Git commit SHA")
    dist.mkdir(parents=True, exist_ok=True)
    for target in TARGETS:
        name = f"na-{target}.tar.gz"
        path = dist / name
        if not path.is_file():
            raise ReleaseError(f"missing build artifact: {name}")
        checksum = (dist / (name + ".sha256")).read_text().split()
        if checksum != [file_digest(path), name]:
            raise ReleaseError(f"invalid build checksum: {name}")
        with tarfile.open(path, "r:gz") as tar:
            members = tar.getmembers()
            if len(members) != 1 or members[0].name != "na" or not members[0].isfile():
                raise ReleaseError(f"invalid build archive: {name}")
    if installer.resolve() != (dist / "install.sh").resolve():
        shutil.copyfile(installer, dist / "install.sh")
    manifest = {
        "schema": 1,
        "tag": tag,
        "commit": commit,
        "assets": {name: file_digest(dist / name) for name in ASSETS},
    }
    (dist / MANIFEST).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return manifest


def load_bundle(dist: Path, tag: str) -> dict:
    validate_tag(tag)
    try:
        manifest = json.loads((dist / MANIFEST).read_text())
        if (
            manifest.get("schema") != 1
            or manifest.get("tag") != tag
            or not re.fullmatch(r"[0-9a-f]{40}", manifest.get("commit", ""))
            or set(manifest.get("assets", {})) != set(ASSETS)
        ):
            raise ReleaseError("invalid release manifest")
        for name in ASSETS:
            if file_digest(dist / name) != manifest["assets"][name]:
                raise ReleaseError(
                    f"local release asset does not match manifest: {name}"
                )
        return manifest
    except (OSError, ValueError, TypeError, AttributeError) as error:
        raise ReleaseError("cannot read a valid release bundle") from error


class ReleasePublisher:
    def __init__(
        self,
        source: str,
        repo: str,
        token: str,
        api_base: str | None = None,
        releases_url: str | None = None,
    ):
        if source not in ("github", "gitee"):
            raise ReleaseError("unknown release source")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
            raise ReleaseError("repository must be OWNER/REPO")
        if not token:
            raise ReleaseError(f"missing {source} release credentials")
        self.source = source
        self.repo = repo
        self.token = token
        base = (
            "https://api.github.com"
            if source == "github"
            else "https://gitee.com/api/v5"
        )
        self.api_base = (api_base or f"{base}/repos/{repo}").rstrip("/")
        self.releases_url = (
            releases_url or f"https://{source}.com/{repo}/releases"
        ).rstrip("/")
        validate_url(self.api_base)
        validate_url(self.releases_url)
        self.api_opener = build_opener(NoRedirect())
        self.download_opener = build_opener(PublicRedirect())

    def api(
        self,
        method: str,
        path: str,
        data: bytes | dict | None = None,
        content_type: str = "application/json",
        url: str | None = None,
    ):
        target = url or self.api_base + path
        validate_url(target)
        origin = urlsplit(self.api_base)
        destination = urlsplit(target)
        if (origin.scheme, origin.netloc) != (
            destination.scheme,
            destination.netloc,
        ) and not (
            self.source == "github"
            and destination.scheme == "https"
            and destination.netloc == "uploads.github.com"
        ):
            raise ReleaseError("refusing to send release credentials to another origin")
        if isinstance(data, dict):
            data = json.dumps(data).encode()
        request = Request(
            target,
            data=data,
            method=method,
            headers={
                "Authorization": f"Bearer {self.token}",
                "Content-Type": content_type,
                "Accept": "application/json",
                "User-Agent": "nano-assistant-release",
            },
        )
        try:
            with self.api_opener.open(request, timeout=120) as response:
                return json.load(response)
        except HTTPError as error:
            status = error.code
            error.close()
            raise ApiError(status) from None
        except (URLError, TimeoutError, ValueError, OSError):
            raise ReleaseError(
                "release API request failed or returned invalid JSON"
            ) from None

    def download_digest(self, url: str) -> str:
        return self.download(url)

    def download(self, url: str, destination: Path | None = None) -> str:
        validate_url(url)
        try:
            request = Request(url, headers={"User-Agent": "nano-assistant-release"})
            with ExitStack() as stack:
                response = stack.enter_context(
                    self.download_opener.open(request, timeout=120)
                )
                validate_url(response.url)
                output = (
                    stack.enter_context(destination.open("wb")) if destination else None
                )
                digest = hashlib.sha256()
                while chunk := response.read(1024 * 1024):
                    digest.update(chunk)
                    if output is not None:
                        output.write(chunk)
                return digest.hexdigest()
        except HTTPError as error:
            error.close()
            raise ReleaseError("anonymous release attachment download failed") from None
        except (URLError, TimeoutError, OSError):
            raise ReleaseError("anonymous release attachment download failed") from None

    def release(self, tag: str, commit: str, notes: str) -> dict:
        try:
            release = self.api("GET", f"/releases/tags/{quote(tag, safe='')}")
        except ApiError as error:
            if error.status != 404:
                raise
            values = {
                "tag_name": tag,
                "target_commitish": commit,
                "name": f"nano-assistant {tag}",
                "body": notes,
                "prerelease": False,
            }
            if self.source == "github":
                values["draft"] = False
            release = self.api("POST", "/releases", values)
        if (
            not isinstance(release, dict)
            or release.get("tag_name") != tag
            or release.get("draft")
            or release.get("prerelease")
            or not isinstance(release.get("id"), int)
        ):
            raise ReleaseError(
                "release is not a public stable version with the expected tag"
            )
        return release

    def assets(self, release_id: int) -> dict:
        endpoint = "assets" if self.source == "github" else "attach_files"
        found = {}
        for page in range(1, 21):
            values = self.api(
                "GET", f"/releases/{release_id}/{endpoint}?per_page=100&page={page}"
            )
            if not isinstance(values, list):
                raise ReleaseError("invalid release asset list")
            for asset in values:
                name = asset.get("name")
                if not isinstance(name, str) or name in found:
                    raise ReleaseError("invalid or duplicate release asset name")
                found[name] = asset
            if len(values) < 100:
                return found
        raise ReleaseError("too many release assets")

    def upload(self, release: dict, path: Path) -> dict:
        data = path.read_bytes()
        if self.source == "github":
            upload_url = release["upload_url"].split("{", 1)[0]
            return self.api(
                "POST",
                "",
                data,
                "application/octet-stream",
                upload_url + "?" + urlencode({"name": path.name}),
            )
        boundary = "nano-assistant-" + uuid.uuid4().hex
        body = (
            f'--{boundary}\r\nContent-Disposition: form-data; name="file"; '
            f'filename="{path.name}"\r\nContent-Type: application/octet-stream\r\n\r\n'
        ).encode()
        body += data + f"\r\n--{boundary}--\r\n".encode()
        return self.api(
            "POST",
            f"/releases/{release['id']}/attach_files",
            body,
            f"multipart/form-data; boundary={boundary}",
        )

    def verify_asset(self, asset: dict, path: Path, tag: str) -> None:
        url = f"{self.releases_url}/download/{quote(tag, safe='')}/{quote(path.name, safe='')}"
        if asset.get("name") != path.name or self.download_digest(url) != file_digest(
            path
        ):
            raise ReleaseError(
                f"published asset conflicts with release bundle: {path.name}"
            )

    def publish(self, dist: Path, tag: str, notes: str) -> None:
        manifest = load_bundle(dist, tag)
        release = self.release(tag, manifest["commit"], notes)
        assets = self.assets(release["id"])
        if MANIFEST in assets and any(name not in assets for name in ASSETS):
            raise ReleaseError(
                "published ready manifest exists but release assets are missing"
            )
        for name in (*ASSETS, MANIFEST):
            path = dist / name
            if name not in assets:
                assets[name] = self.upload(release, path)
            self.verify_asset(assets[name], path, tag)
        print(
            f"Verified {self.source} release {tag}: {len(ASSETS) + 1} identical public assets"
        )

    def recover(self, dist: Path, tag: str, commit: str) -> None:
        validate_tag(tag)
        release = self.api("GET", f"/releases/tags/{quote(tag, safe='')}")
        if (
            release.get("tag_name") != tag
            or release.get("draft")
            or release.get("prerelease")
        ):
            raise ReleaseError("recovery requires an existing public stable release")
        assets = self.assets(release["id"])
        if any(name not in assets for name in ASSETS):
            raise ReleaseError(
                "recovery source is incomplete; rerun the failed workflow jobs instead"
            )
        dist.mkdir(parents=True, exist_ok=True)
        for name in ASSETS:
            self.download(assets[name]["browser_download_url"], dist / name)
        if MANIFEST in assets:
            self.download(assets[MANIFEST]["browser_download_url"], dist / MANIFEST)
            manifest = load_bundle(dist, tag)
            if manifest["commit"] != commit:
                raise ReleaseError("recovery source commit does not match the Git tag")
        else:
            prepare_bundle(dist, tag, commit, dist / "install.sh")
        (dist / "release-notes.md").write_text(release.get("body") or "")
        print(f"Recovered original {self.source} assets for {tag}; no rebuild")


def sync_repository(
    tag: str,
    destination: str,
    username: str = "oauth2",
    branch: str = "main",
    cwd: Path | None = None,
) -> None:
    validate_tag(tag)
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", branch):
        raise ReleaseError("invalid mirror branch")
    parsed = urlsplit(destination)
    if parsed.scheme != "file":
        validate_url(destination)
    env = {**os.environ, "GIT_TERMINAL_PROMPT": "0", "GITEE_USERNAME": username}
    helper = '!f() { if [ "$1" = get ]; then printf "username=%s\npassword=%s\n" "$GITEE_USERNAME" "$GITEE_TOKEN"; fi; }; f'

    def git(*args: str) -> str:
        result = subprocess.run(
            [
                "git",
                "-c",
                "credential.helper=",
                "-c",
                f"credential.helper={helper}",
                *args,
            ],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
        )
        if result.returncode:
            raise ReleaseError(
                "Git mirror operation failed; check credentials, connectivity and fast-forward history"
            )
        return result.stdout.strip()

    commit = git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
    refs = git("ls-remote", destination, f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}")
    remote_refs = dict(line.split()[::-1] for line in refs.splitlines())
    existing = remote_refs.get(
        f"refs/tags/{tag}^{{}}", remote_refs.get(f"refs/tags/{tag}")
    )
    if existing and existing != commit:
        raise ReleaseError("Gitee tag already points to a different release commit")
    if not existing:
        git("push", destination, f"refs/tags/{tag}:refs/tags/{tag}")
    git("push", destination, f"refs/remotes/origin/{branch}:refs/heads/{branch}")
    print(f"Verified mirror tag {tag}; synchronized {branch} without force push")


def main() -> None:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--tag", required=True)
    prepare.add_argument("--commit", required=True)
    prepare.add_argument("--dist", type=Path, default=Path("dist"))
    prepare.add_argument("--installer", type=Path, default=Path("install.sh"))
    publish = commands.add_parser("publish")
    publish.add_argument("--source", choices=("github", "gitee"), required=True)
    publish.add_argument("--tag", required=True)
    publish.add_argument("--dist", type=Path, default=Path("dist"))
    publish.add_argument("--notes-file", type=Path, required=True)
    recover = commands.add_parser("recover")
    recover.add_argument("--source", choices=("github", "gitee"), default="github")
    recover.add_argument("--tag", required=True)
    recover.add_argument("--commit", required=True)
    recover.add_argument("--dist", type=Path, default=Path("dist"))
    sync = commands.add_parser("sync")
    sync.add_argument("--tag", required=True)
    sync.add_argument(
        "--repo", default=os.environ.get("GITEE_REPO", "arcat00/nano-assistant")
    )
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            prepare_bundle(args.dist, args.tag, args.commit, args.installer)
        elif args.command == "sync":
            if not os.environ.get("GITEE_TOKEN"):
                raise ReleaseError("missing Gitee mirror credentials")
            if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repo):
                raise ReleaseError("repository must be OWNER/REPO")
            sync_repository(
                args.tag, f"https://gitee.com/{args.repo}.git", args.repo.split("/")[0]
            )
        else:
            repo = (
                os.environ.get("GITHUB_REPOSITORY", "arcat0v0/nano-assistant")
                if args.source == "github"
                else os.environ.get("GITEE_REPO", "arcat00/nano-assistant")
            )
            token = os.environ.get(
                "GH_TOKEN" if args.source == "github" else "GITEE_TOKEN", ""
            )
            publisher = ReleasePublisher(args.source, repo, token)
            if args.command == "recover":
                publisher.recover(args.dist, args.tag, args.commit)
            else:
                publisher.publish(args.dist, args.tag, args.notes_file.read_text())
    except (ReleaseError, OSError, tarfile.TarError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
