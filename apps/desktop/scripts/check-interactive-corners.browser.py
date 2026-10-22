"""Check the compiled desktop stylesheet's interactive-radius boundary in Chrome."""
import argparse
import json
from playwright.sync_api import sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument("--url", default="http://127.0.0.1:1420/")
args = parser.parse_args()

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, channel="chrome")
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    page.goto(args.url, wait_until="domcontentloaded", timeout=60000)
    page.wait_for_function("getComputedStyle(document.documentElement).getPropertyValue('--ui-interactive-radius').trim() !== ''", timeout=60000)
    values = page.evaluate("""() => {
      const cases = [
        ['button', 'button', '', 'rounded-lg'],
        ['menu', 'div', 'menuitem', 'rounded-lg'],
        ['radio', 'div', 'menuitemradio', 'rounded-lg'],
        ['checkbox', 'div', 'menuitemcheckbox', 'rounded-lg'],
        ['option', 'div', 'option', 'rounded-md'],
        ['tab', 'button', 'tab', 'rounded-md'],
        ['roleButton', 'div', 'button', 'rounded-sm'],
        ['row', 'div', '', 'cursor-pointer rounded-md'],
        ['highlight', 'div', '', 'ui-hover-highlight rounded-md'],
        ['dataMenu', 'div', '', 'rounded-xl'],
        ['capsule', 'button', '', 'rounded-full'],
        ['square', 'button', '', 'rounded-none'],
        ['large', 'button', '', 'rounded-2xl'],
        ['card', 'div', '', 'rounded-md'],
        ['field', 'input', '', 'rounded-md'],
      ];
      const fixture = document.createElement('div');
      fixture.style.cssText = 'position:fixed;left:0;top:0;z-index:99999;pointer-events:none';
      document.body.append(fixture);
      const result = {};
      for (const [id, tag, role, classes] of cases) {
        const node = document.createElement(tag);
        node.className = classes;
        node.style.cssText = 'width:120px;height:40px';
        if (role) node.setAttribute('role', role);
        if (id === 'dataMenu') node.setAttribute('data-menu-item', 'fixture');
        fixture.append(node);
        const style = getComputedStyle(node);
        result[id] = { radius: parseFloat(style.borderTopLeftRadius), shape: style.getPropertyValue('corner-shape') };
      }
      result.scale = Number(getComputedStyle(document.documentElement).getPropertyValue('--continuous-corner-scale'));
      result.supported = CSS.supports('corner-shape', 'superellipse(2)');
      // A missing theme token must not turn a valid rounded highlight square.
      const root = document.documentElement;
      const savedRadius = root.style.getPropertyValue('--ui-interactive-radius');
      root.style.setProperty('--ui-interactive-radius', 'initial');
      const fallback = fixture.querySelector('.ui-hover-highlight');
      result.missingTokenRadius = parseFloat(getComputedStyle(fallback).borderTopLeftRadius);
      if (savedRadius) root.style.setProperty('--ui-interactive-radius', savedRadius);
      else root.style.removeProperty('--ui-interactive-radius');
      const selection = document.querySelector('.approval-selection');
      if (selection) {
        result.preview = {
          radius: parseFloat(getComputedStyle(selection).borderTopLeftRadius),
          rowRadius: parseFloat(getComputedStyle(document.querySelector('.approval-option')).borderTopLeftRadius)
        };
      }
      fixture.remove();
      return result;
    }""")
    scale = values["scale"]
    assert scale > 0, "The desktop corner policy has not loaded"
    assert abs(values["missingTokenRadius"] - 12 * scale) < .01, values["missingTokenRadius"]
    for key in ["button", "menu", "radio", "checkbox", "option", "tab", "roleButton", "row", "highlight", "dataMenu"]:
        assert abs(values[key]["radius"] - 12 * scale) < .01, (key, values[key])
        if values["supported"]:
            assert values[key]["shape"].startswith("superellipse("), (key, values[key])
    for key in ["capsule"]:
        assert values[key]["radius"] >= 999, (key, values[key])
        if values["supported"]:
            assert values[key]["shape"] in ["round", "superellipse(1)"], (key, values[key])
    assert values["square"]["radius"] == 0
    assert abs(values["large"]["radius"] - 16 * scale) < .01
    for key in ["card", "field"]:
        assert abs(values[key]["radius"] - 6 * scale) < .01, (key, values[key])
    if "preview" in values:
        assert abs(values["preview"]["radius"] - 12 * scale) < .01
        assert abs(values["preview"]["rowRadius"] - values["preview"]["radius"]) < .01
    print(json.dumps({"interactiveCorners": "passed", "cases": 15, "nativeContinuousCorners": values["supported"], "missingTokenFallback": "passed", "capsulesAndSquares": "preserved", "staticCardsAndFields": "preserved", "preview": values.get("preview")}, ensure_ascii=False))
    browser.close()
