"""Regression for the reference-style single-shell menu morph. Run against a local Vite server.

Requires Python Playwright and Chrome. Browser settings are stubbed in an isolated
browser context; no API key, native command, or production setting is changed.
"""
import argparse
import json
import re
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--url", default="http://127.0.0.1:1421/")
parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / ".tmp/menu-trigger-regression")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)


def preview_settings(route):
    response = route.fetch()
    source, count = re.subn(
        r'(async function getSettings\(\)\s*\{.*?return )null;',
        r'\1{configured:true,available_models:["deepseek-chat"],model_capabilities:[],chat_model:"deepseek-chat",reasoner_model:"deepseek-reasoner"};',
        response.text(), count=1, flags=re.S,
    )
    assert count == 1, "Preview settings boundary changed; update the test fixture"
    route.fulfill(response=response, body=source)


with sync_playwright() as playwright:
    browser = playwright.chromium.launch(headless=True, channel="chrome")
    page = browser.new_page(viewport={"width": 1440, "height": 1000})
    page.set_default_timeout(5000)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.route("**/src/api.ts", preview_settings)
    page.goto(args.url)
    page.get_by_role("heading", name="我们应该做些什么？").wait_for()
    expect(page.get_by_role("textbox", name="API Key", exact=True)).to_have_count(0)
    page.wait_for_load_state("networkidle")
    selectors = [re.compile("deepseek-chat|选择模型"), "请求批准", "不使用项目", "本地模式"]
    results = []
    for name in selectors:
        trigger = page.get_by_role("button", name=name, exact=isinstance(name, str))
        label = trigger.text_content()
        motion_id = trigger.locator('xpath=ancestor::*[@data-motion-id][1]').get_attribute('data-motion-id')
        shell = page.locator(f'[data-motion-id={json.dumps(motion_id)}]')
        origin = shell.bounding_box()
        shell.evaluate("""node => {
          window.morphNode = node; window.morphFrames = []; window.morphDone = false;
          const start = performance.now();
          function sample(now) {
            const r = node.getBoundingClientRect();
            window.morphFrames.push({time: now-start, x:r.x, y:r.y, width:r.width, height:r.height, bottom:r.bottom,
              sameShell:node===window.morphNode, opacity:getComputedStyle(node).opacity});
            if(now-start<1250) requestAnimationFrame(sample); else window.morphDone=true;
          }
          requestAnimationFrame(sample);
        }""")
        trigger.click()
        page.wait_for_function("window.morphDone")
        frames = page.evaluate("window.morphFrames")
        assert shell.evaluate('(node)=>node===window.morphNode'), "Button was replaced with a separate popup"
        expanded = shell.bounding_box()
        panel = shell.locator('[data-morph-panel]')
        panel_rect = panel.bounding_box()
        page.screenshot(path=str(args.output / f'menu-{len(results)}.png'))
        assert abs(expanded['y'] + expanded['height'] - origin['y'] - origin['height']) < 1, ("Expanded card left a gap at its original button",label,origin,expanded)
        assert abs(panel_rect['height'] - expanded['height']) < 1
        assert all(frame['sameShell'] and float(frame['opacity']) == 1 for frame in frames)
        assert all(abs(frame['bottom'] - origin['y'] - origin['height']) < 1 for frame in frames)
        heights = [frame['height'] for frame in frames]
        assert max(heights) > expanded['height'] + 2, "Reference rebound was lost"
        expect(shell.locator('.morph-menu-compact')).to_have_attribute('aria-hidden','true')
        expect(panel.locator('.morph-menu-current')).to_have_count(0)
        expect(panel.get_by_role('button',name='收起菜单')).to_have_count(0)
        assert panel.evaluate('node => { const s = getComputedStyle(node); return s.outlineStyle === "none" || parseFloat(s.outlineWidth) === 0 || s.outlineColor === "rgba(0, 0, 0, 0)"; }'), "The menu container added a visible focus frame"
        assert panel.evaluate('node => node.children.length === 1 && node.firstElementChild.classList.contains("morph-menu-content")'), "An extra row was added around the original menu"
        assert abs(panel_rect['height'] - panel.locator('.morph-menu-content').bounding_box()['height']) < 1, "An extra header changed the original panel layout"
        page.screenshot(path=str(args.output / f'menu-{len(results)}.png'))
        # The expanded face folds back into exactly the same DOM shell and slot.
        page.keyboard.press('Escape')
        expect(panel).to_have_count(0)
        returned = shell.bounding_box()
        for key in ['x','y','width','height']:
            assert abs(returned[key]-origin[key]) < 1, (label,key,origin,returned)
        expect(shell.locator('.morph-menu-compact button')).to_be_visible()
        expect(shell.locator('.morph-menu-compact button')).to_be_focused()
        # Reverse closing mid-flight by clicking the original compact slot.
        shell.locator('.morph-menu-compact button').click()
        page.wait_for_timeout(1000)
        page.keyboard.press('Escape')
        page.wait_for_timeout(85)
        page.mouse.click(origin['x']+origin['width']/2,origin['y']+origin['height']/2)
        page.wait_for_timeout(1000)
        expect(shell).to_have_attribute('data-state','open')
        assert abs(shell.bounding_box()['height']-expanded['height']) < 1
        page.keyboard.press('Escape')
        expect(panel).to_have_count(0)
        results.append({'trigger':label,'onePersistentShell':True,'originalPanelOnly':True,'closedHeight':origin['height'],
            'expandedHeight':expanded['height'],'peakHeight':max(heights),'roundTrip':'passed'})
    page.emulate_media(reduced_motion='reduce')
    page.get_by_role('button',name='请求批准',exact=True).click()
    page.mouse.click(1250, 100)
    expect(page.locator('[data-morph-panel]')).to_have_count(0)
    expect(page.get_by_role('button',name='请求批准',exact=True)).to_be_visible()
    assert not errors, errors
    (args.output/'results.json').write_text(json.dumps(results,ensure_ascii=False,indent=2),encoding='utf8')
    print(json.dumps({'menus':results,'reducedMotion':'passed','pageErrors':errors},ensure_ascii=False))
    browser.close()
