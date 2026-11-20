"""Reviewed POSIX archive helper, invoked with one base64 JSON argument.

Only regular files and directories are accepted. All destination IO uses an
anchored directory descriptor and O_NOFOLLOW; archive paths never become shell.
"""
import base64
import hashlib
import json
import os
import signal
import stat
import struct
import sys
import tarfile
import uuid
import zipfile

MAX_ENTRIES = 10000
MAX_BYTES = 512 * 1024 * 1024
FLAGS = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)


def require(value, message):
    if not value:
        raise ValueError(message)


def parts(name):
    require(isinstance(name, str) and 0 < len(name.encode()) <= 16384, "invalid entry name")
    require(not name.startswith("/") and "\\" not in name and "\0" not in name,
            "absolute or non-POSIX archive path")
    result = name.rstrip("/").split("/")
    require(all(part not in ("", ".", "..") and ":" not in part for part in result), "archive path traversal")
    require(len(result) <= 64, "archive path depth exceeds 64")
    return result


def absolute_directory(path):
    require(isinstance(path, str) and path.startswith("/") and "\0" not in path, "absolute directory required")
    components = path[1:].split("/") if path != "/" else []
    require(all(p not in ("", ".", "..") for p in components), "invalid directory components")
    fd = os.open("/", FLAGS)
    try:
        for part in components:
            child = os.open(part, FLAGS, dir_fd=fd)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def parent_fd(root, components, create=False):
    fd = os.dup(root)
    try:
        for part in components:
            try:
                child = os.open(part, FLAGS, dir_fd=fd)
            except FileNotFoundError:
                if not create:
                    os.close(fd)
                    return None
                os.mkdir(part, 0o700, dir_fd=fd)
                child = os.open(part, FLAGS, dir_fd=fd)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def sha256(stream, limit=MAX_BYTES):
    digest = hashlib.sha256()
    count = 0
    while True:
        block = stream.read(65536)
        if not block:
            break
        count += len(block)
        require(count <= limit, "archive input or comparison exceeds byte quota")
        digest.update(block)
    return digest.hexdigest()


def target_fact(root, name, budget):
    components = parts(name)
    fd = parent_fd(root, components[:-1])
    if fd is None:
        return None
    try:
        try:
            info = os.stat(components[-1], dir_fd=fd, follow_symlinks=False)
        except FileNotFoundError:
            return None
        require(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode), "destination contains link or special file")
        result = [info.st_dev, info.st_ino, info.st_mode]
        if stat.S_ISREG(info.st_mode):
            budget[0] += info.st_size
            require(budget[0] <= MAX_BYTES, "destination comparison exceeds byte quota")
            with os.fdopen(os.open(components[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=fd), "rb") as stream:
                opened = os.fstat(stream.fileno())
                require((opened.st_dev, opened.st_ino) == (info.st_dev, info.st_ino), "destination changed during preview")
                result.extend([info.st_size, info.st_mtime_ns, sha256(stream)])
        return result
    finally:
        os.close(fd)


def delete_stage(fd):
    for name in os.listdir(fd):
        info = os.stat(name, dir_fd=fd, follow_symlinks=False)
        if stat.S_ISDIR(info.st_mode):
            child = os.open(name, FLAGS, dir_fd=fd)
            try:
                delete_stage(child)
            finally:
                os.close(child)
            os.rmdir(name, dir_fd=fd)
        else:
            os.unlink(name, dir_fd=fd)


def bounded_zip_directory(source):
    """Bound allocation before ZipFile eagerly reads the central directory.

    This format deliberately rejects ZIP64/multidisk and trailing payloads;
    oversized/ambiguous containers need a different reviewed extractor.
    """
    source.seek(0, os.SEEK_END)
    length = source.tell()
    source.seek(max(0, length - 65557))
    tail = source.read(65557)
    offset = tail.rfind(b"PK\x05\x06")
    require(offset >= 0 and len(tail) - offset >= 22, "invalid zip end record")
    disk, start_disk, disk_entries, total_entries, central_size, central_offset, comment_size = struct.unpack_from("<4H2IH", tail, offset + 4)
    require(offset + 22 + comment_size == len(tail), "zip trailing payload or invalid comment")
    require(disk == start_disk == 0 and disk_entries == total_entries, "multidisk zip unsupported")
    require(total_entries <= MAX_ENTRIES and total_entries != 65535 and central_size <= 4 * 1024 * 1024,
            "zip central directory exceeds quota or requires ZIP64")
    require(central_offset + central_size == length - len(tail) + offset, "zip central directory layout unsupported")
    source.seek(0)


def main(request):
    require(os.name == "posix" and hasattr(os, "O_NOFOLLOW") and os.open in os.supports_dir_fd,
            "unsupported: safe extraction needs POSIX directory descriptors and O_NOFOLLOW")
    require(request.get("action") in ("preview", "extract"), "unknown archive action")
    require(request.get("conflict", "ask") in ("ask", "skip", "replace"), "invalid archive conflict policy")
    archive_path = request["archivePath"]
    require(isinstance(archive_path, str) and archive_path.startswith("/"), "absolute archive path required")
    archive_parent, archive_name = os.path.split(archive_path)
    parts(archive_name)
    archive_fd = absolute_directory(archive_parent or "/")
    root = absolute_directory(request["destinationPath"])
    stage_name = None
    stage = None
    receipt = {"files": 0, "bytes": "0", "skipped": 0, "destinationPath": request["destinationPath"]}
    try:
        descriptor = os.open(archive_name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=archive_fd)
        with os.fdopen(descriptor, "rb") as source:
            info = os.fstat(source.fileno())
            require(stat.S_ISREG(info.st_mode) and info.st_size <= MAX_BYTES, "archive exceeds input quota or is not a file")
            archive_hash = sha256(source)
            source.seek(0)
            is_zip = zipfile.is_zipfile(source)
            source.seek(0)
            if is_zip:
                bounded_zip_directory(source)
            archive = zipfile.ZipFile(source) if is_zip else tarfile.open(fileobj=source, mode="r:*")
            with archive:
                entries = {}
                members = {}
                declared = 0
                count = 0
                metadata_bytes = 0
                for item in archive.infolist() if is_zip else archive:
                    count += 1
                    require(count <= MAX_ENTRIES, "archive exceeds entry quota")
                    name = item.orig_filename if is_zip else item.name
                    components = parts(name)
                    metadata_bytes += len(name.encode())
                    require(metadata_bytes <= 2 * 1024 * 1024, "archive path metadata exceeds quota")
                    canonical = "/".join(components)
                    directory = item.is_dir() if is_zip else item.isdir()
                    if is_zip:
                        kind = stat.S_IFMT(item.external_attr >> 16)
                        require(kind in (0, stat.S_IFREG, stat.S_IFDIR) and not item.flag_bits & 1, "zip links, special or encrypted entries unsupported")
                        require(kind != stat.S_IFDIR or directory, "zip file/directory type mismatch")
                        size = item.file_size
                    else:
                        require((item.isfile() or directory) and not item.sparse, "tar links, sparse or special entries unsupported")
                        size = item.size
                    require(canonical not in members, "duplicate archive path")
                    require(size >= 0 and (not directory or size == 0), "invalid archive size")
                    declared += size
                    require(declared <= MAX_BYTES, "archive exceeds uncompressed byte quota")
                    for length in range(1, len(components)):
                        ancestor = "/".join(components[:length])
                        require(ancestor not in entries or entries[ancestor]["directory"], "file used as archive directory")
                        entries.setdefault(ancestor, {"path": ancestor, "directory": True, "sizeBytes": "0"})
                    require(canonical not in entries or directory and entries[canonical]["directory"], "archive file/directory collision")
                    entries[canonical] = {"path": canonical, "directory": directory, "sizeBytes": str(size)}
                    members[canonical] = item
                    require(len(entries) <= MAX_ENTRIES, "implicit directories exceed entry quota")
                ordered = sorted(entries.values(), key=lambda entry: (entry["path"].count("/"), entry["path"]))
                budget = [0]
                conflicts = 0
                for entry in ordered:
                    # Never replace the archive being read.
                    require(os.path.normpath(request["destinationPath"] + "/" + entry["path"]) != archive_path, "archive would replace itself")
                    fact = target_fact(root, entry["path"], budget)
                    entry["existing"] = fact
                    if fact:
                        require(stat.S_ISDIR(fact[2]) == entry["directory"], "destination file/directory conflict")
                        conflicts += not entry["directory"]
                root_info = os.fstat(root)
                facts = {"archiveSha256": archive_hash, "destinationPath": request["destinationPath"], "root": [root_info.st_dev, root_info.st_ino], "entries": ordered, "conflict": request.get("conflict", "ask")}
                fingerprint = hashlib.sha256(json.dumps(facts, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                preview = {"version": 1, "archivePath": archive_path, "destinationPath": request["destinationPath"], "format": "zip" if is_zip else "tar", "entries": ordered, "totalBytes": str(declared), "conflicts": conflicts, "fingerprint": fingerprint, "maxEntries": MAX_ENTRIES, "maxBytes": str(MAX_BYTES)}
                if request["action"] == "preview":
                    return {"ok": True, "preview": preview}
                require(request.get("fingerprint") == fingerprint, "conflict: archive or destination changed; preview again")
                require(not conflicts or request.get("conflict") != "ask", "conflict: choose skip or replace")
                stage_name = ".deepagent-extract-" + uuid.uuid4().hex
                os.mkdir(stage_name, 0o700, dir_fd=root)
                stage = os.open(stage_name, FLAGS, dir_fd=root)
                actual = 0
                for entry in ordered:
                    components = parts(entry["path"])
                    fd = parent_fd(stage, components[:-1], True)
                    try:
                        if entry["directory"]:
                            try:
                                os.mkdir(components[-1], 0o700, dir_fd=fd)
                            except FileExistsError:
                                pass
                            continue
                        descriptor = os.open(components[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=fd)
                        item = members[entry["path"]]
                        stream = archive.open(item) if is_zip else archive.extractfile(item)
                        written = 0
                        with stream, os.fdopen(descriptor, "wb") as output:
                            while True:
                                block = stream.read(65536)
                                if not block:
                                    break
                                actual += len(block)
                                written += len(block)
                                require(actual <= MAX_BYTES and written <= int(entry["sizeBytes"]), "expanded data exceeds quota or declared size")
                                output.write(block)
                        require(written == int(entry["sizeBytes"]), "archive entry was truncated")
                    finally:
                        os.close(fd)
                require(sha256_again(source) == archive_hash, "archive changed while extracting")
                for entry in ordered:
                    current = target_fact(root, entry["path"], [0])
                    require(current == entry["existing"], "conflict: destination changed before publication")
                    components = parts(entry["path"])
                    dest_fd = parent_fd(root, components[:-1], True)
                    try:
                        if entry["directory"]:
                            if not entry["existing"]:
                                os.mkdir(components[-1], 0o700, dir_fd=dest_fd)
                            continue
                        if entry["existing"] and request["conflict"] == "skip":
                            receipt["skipped"] += 1
                            continue
                        src_fd = parent_fd(stage, components[:-1])
                        try:
                            if entry["existing"]:
                                os.replace(components[-1], components[-1], src_dir_fd=src_fd, dst_dir_fd=dest_fd)
                            else:
                                os.link(components[-1], components[-1], src_dir_fd=src_fd, dst_dir_fd=dest_fd, follow_symlinks=False)
                                os.unlink(components[-1], dir_fd=src_fd)
                        finally:
                            os.close(src_fd)
                        receipt["files"] += 1
                        receipt["bytes"] = str(int(receipt["bytes"]) + int(entry["sizeBytes"]))
                    finally:
                        os.close(dest_fd)
                return {"ok": True, "receipt": receipt}
    except Exception as error:
        return {"ok": False, "error": str(error)[:4096], "receipt": receipt}
    finally:
        if stage is not None:
            try:
                delete_stage(stage)
                os.rmdir(stage_name, dir_fd=root)
            except Exception as error:
                receipt["cleanupWarning"] = str(error)[:4096]
            os.close(stage)
        os.close(root)
        os.close(archive_fd)


def sha256_again(source):
    source.seek(0)
    return sha256(source)


if __name__ == "__main__":
    try:
        if hasattr(signal, "alarm"):
            def stop(signum, frame):
                raise ValueError("archive helper interrupted or timed out; review partial destination")
            signal.signal(signal.SIGALRM, stop)
            signal.signal(signal.SIGHUP, stop)
            signal.alarm(150)
        request = json.loads(base64.b64decode(sys.argv[1], validate=True))
        print(json.dumps(main(request), ensure_ascii=True, separators=(",", ":")))
    except Exception as error:
        print(json.dumps({"ok": False, "error": str(error)[:4096]}))
