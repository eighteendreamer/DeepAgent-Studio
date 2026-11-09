"""Read-only checks for this architecture document set; no product tests."""
from pathlib import Path
import argparse
import hashlib
import json
import re
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parents[3]
diagram_dir = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, help="Optional JSON result outside the repository")
output = parser.parse_args().output
documents = [
    root / "docs/2026-11-09_linux-vm-architecture-upgrade-report.md",
    root / "docs/linux-vm-contracts-and-storage-policy-v1.md",
    root / "docs/linux-vm-evidence-and-acceptance.md",
    root / "docs/linux-vm-implementation-blueprint.md",
    diagram_dir / "README.md",
]
names = ["architecture", "components", "execution", "workspace", "storage", "maintenance", "lifecycle"]
report = {"scope": "documentation only", "documents": [], "diagrams": []}
for document in documents:
    text = document.read_text(encoding="utf-8")
    assert "\ufffd" not in text, (document, "invalid replacement character")
    assert len(re.findall(r"^```.*$", text, re.M)) % 2 == 0, (document, "unpaired fence")
    assert not re.search(r"\bTODO\b|待补充|此处填写", text), document
    targets = re.findall(r"\]\((<[^>]+>|[^\n)]+)\)", text)
    local_count = 0
    for target in targets:
        target = target.strip("<>").split("#", 1)[0]
        if not target or re.match(r"https?://", target):
            continue
        path = Path(target) if re.match(r"^[A-Za-z]:[/\\]", target) else document.parent / target
        is_output = output is not None and path.resolve() == output.resolve()
        assert path.exists() or is_output, (document.name, "missing link", target)
        local_count += 1
    report["documents"].append({"path": str(document.relative_to(root)), "localLinks": local_count,
        "lines": len(text.splitlines()), "sha256": hashlib.sha256(text.encode()).hexdigest()})

main = documents[0].read_text(encoding="utf-8")
inline = re.findall(r"^```mermaid\n(.*?)^```", main, re.M | re.S)
assert len(inline) == 2
for name, source in zip(["architecture", "execution"], inline):
    assert source.strip() == (diagram_dir / f"{name}.mmd").read_text(encoding="utf-8").strip(), name

blueprint = documents[3].read_text(encoding="utf-8")
units = re.findall(r"^### (UP-\d\d)：", blueprint, re.M)
assert units == [f"UP-{number:02d}" for number in range(13)], units
interfaces = re.findall(r"^\| (I\d\d) \|", blueprint, re.M)
assert interfaces == [f"I{number:02d}" for number in range(1, 10)], interfaces
budget_mb = [350, 60, 1000, 1000, 100, 300, 190]
assert sum(budget_mb) == 3000
for language, body in re.findall(r"^```([^\n]+)\n(.*?)^```", documents[1].read_text(encoding="utf-8"), re.M | re.S):
    if language == "json":
        assert json.loads(body)["storage"]["max_linux_pool_bytes"] == 3_000_000_000
for name in names:
    source = diagram_dir / f"{name}.mmd"
    svg = diagram_dir / f"{name}.svg"
    element = ET.fromstring(svg.read_text(encoding="utf-8"))
    assert element.tag.endswith("svg") and element.attrib.get("viewBox"), name
    report["diagrams"].append({"name": name, "sourceSha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "svgSha256": hashlib.sha256(svg.read_bytes()).hexdigest(), "viewBox": element.attrib["viewBox"]})
html = (diagram_dir / "index.html").read_text(encoding="utf-8")
assert len(re.findall(r"<article id=", html)) == 7
assert not re.search(r'<(?:script|link)[^>]+(?:src|href)=["\']https?://', html)
report.update({"inlineSources": "identical", "upgradeUnits": units, "interfaces": interfaces,
    "budgetMB": sum(budget_mb), "htmlSections": 7, "result": "passed"})
encoded = json.dumps(report, ensure_ascii=False, indent=2)
if output:
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(encoded + "\n", encoding="utf-8")
print(encoded)
