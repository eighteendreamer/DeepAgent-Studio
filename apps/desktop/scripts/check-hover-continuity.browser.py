"""Real UI regression checks; run Vite on 1420 before starting this script.
Only the unconfigured settings boundary is replaced, with no external requests.
"""
import json
import os
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

folder = Path(__file__).resolve().parent.parent / '.tmp' / 'hover-continuity'
folder.mkdir(parents=True, exist_ok=True)
url = os.environ.get('DEEPAGENT_PREVIEW_URL', 'http://127.0.0.1:1420/')


def settings(route):
    response = route.fetch()
    source, count = re.subn(
        r'(async function getSettings\(\)\s*\{.*?return )null;',
        r'\1{configured:true,available_models:["deepseek-flash"],model_capabilities:[],chat_model:"deepseek-flash",reasoner_model:"deepseek-flash"};',
        response.text(), count=1, flags=re.S)
    assert count == 1
    route.fulfill(response=response, body=source)


with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, channel='chrome')
    page = browser.new_page(viewport={'width': 1440, 'height': 1000})
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.route('**/src/api.ts', settings)
    page.goto(url, wait_until='commit', timeout=60000)
    page.locator('.composer-textarea').wait_for(timeout=60000)
    rail = page.locator('.app-navigation-rail')
    hint = page.locator('[data-hover-info-continuous]')
    first = rail.locator('[data-nav=new-chat]')
    first.hover()
    page.wait_for_timeout(100)
    expect(hint).to_have_count(0)
    expect(hint).to_be_visible(timeout=2000)
    page.wait_for_timeout(200)
    hint.evaluate('''node=>{
      window.firstHint=node;window.hintFrames=[];window.trackHint=true;const start=performance.now();
      const sample=()=>{const n=document.querySelector('[data-hover-info-continuous]');const r=n?.getBoundingClientRect();
        window.hintFrames.push({t:performance.now()-start,same:n===window.firstHint,
          text:n?.textContent,opacity:n?Number(getComputedStyle(n).opacity):0,x:r?.x,y:r?.y});
        if(window.trackHint)requestAnimationFrame(sample);
      };requestAnimationFrame(sample);
    }''')
    # Pause in the gap longer than the old closeDelay: the shared hint stays open.
    a = first.bounding_box()
    next_button = rail.locator('[data-nav=search]')
    b = next_button.bounding_box()
    page.mouse.move(a['x'] + a['width']/2, (a['y'] + a['height'] + b['y'])/2)
    page.wait_for_timeout(260)
    for item, text in [('search', '搜索'), ('skills', '技能'), ('knowledge', '知识库')]:
        target = rail.locator(f'[data-nav={item}]')
        target.hover()
        expect(hint).to_have_text(text, timeout=200)
        page.wait_for_timeout(190)
        rect = hint.bounding_box()
        button_rect = target.bounding_box()
        assert rect['x'] >= button_rect['x'] + button_rect['width'], item
        assert abs((rect['y'] + rect['height']/2) - (button_rect['y'] + button_rect['height']/2)) < 2, item
    page.evaluate('window.trackHint=false')
    frames = page.evaluate('window.hintFrames')
    assert len(frames) > 20
    assert all(frame['same'] and frame['opacity'] >= .99 for frame in frames), 'Hint remounted or replayed entrance'
    (folder/'hint-frames.json').write_text(json.dumps(frames, indent=2), encoding='utf-8')
    page.screenshot(path=str(folder/'continuous-hint.png'), full_page=True)
    page.mouse.move(700, 200)
    expect(hint).to_have_count(0, timeout=1500)

    # The same grouping also applies to the horizontal window action toolbar.
    page.get_by_role('button', name='打开右侧栏', exact=True).hover()
    expect(hint).to_be_visible(timeout=2000)
    page.wait_for_timeout(200)
    hint.evaluate('node=>{window.toolbarHint=node}')
    page.get_by_role('button', name='打开底部终端', exact=True).hover()
    expect(hint).to_have_text('打开底部终端', timeout=200)
    assert hint.evaluate('node=>node===window.toolbarHint && Number(getComputedStyle(node).opacity)>=.99')
    page.mouse.move(700, 200)
    expect(hint).to_have_count(0, timeout=1500)

    # Keyboard focus updates the same surface, and Escape dismisses it.
    first.focus()
    expect(hint).to_be_visible(timeout=2000)
    page.wait_for_timeout(220)
    hint.evaluate('node=>{window.keyboardHint=node}')
    next_button.focus()
    expect(hint).to_have_text('搜索', timeout=300)
    assert hint.evaluate('node=>node===window.keyboardHint')
    page.keyboard.press('Escape')
    expect(hint).to_have_count(0, timeout=1500)

    borders = {}

    def border(node, name):
        result = node.evaluate('node=>{const s=getComputedStyle(node);return {width:s.outlineWidth,color:s.outlineColor,offset:s.outlineOffset,border:s.borderTopWidth}}')
        assert result['width'] == '1px' and result['offset'] == '-1px', (name, result)
        assert result['color'] != 'rgba(0, 0, 0, 0)', (name, result)
        borders[name] = result

    for width in [1440, 768, 375]:
        page.set_viewport_size({'width': width, 'height': 1000})
        page.mouse.move(width-20, 300)
        trigger = rail.locator('button[aria-label="设置"]')
        start = trigger.bounding_box()
        trigger.click()
        shell = rail.locator('[data-motion-id^="sidebar-settings-"]')
        panel = shell.locator('[data-morph-panel]')
        expect(panel).to_be_visible()
        page.wait_for_timeout(1000)
        rect = shell.bounding_box()
        assert rect['x'] >= 0 and rect['x'] + rect['width'] <= width, (width, rect)
        expect(hint).to_have_count(0)
        border(panel, f'settings-{width}')
        page.screenshot(path=str(folder/f'{width}-settings-open.png'), full_page=True)
        shell.evaluate('''node=>{
          window.closeFrames=[];window.trackClose=true;const start=performance.now();
          const sample=()=>{const r=node.getBoundingClientRect(),s=getComputedStyle(node);
            window.closeFrames.push({t:performance.now()-start,state:node.getAttribute('data-state'),
              x:r.x,y:r.y,width:r.width,height:r.height,radius:parseFloat(s.borderTopLeftRadius),background:s.backgroundColor,panel:!!node.querySelector('[data-morph-panel]')});
            if(window.trackClose)requestAnimationFrame(sample);
          };requestAnimationFrame(sample);
        }''')
        page.keyboard.press('Escape')
        page.wait_for_timeout(210)
        page.screenshot(path=str(folder/f'{width}-settings-closing.png'), full_page=True)
        expect(panel).to_have_count(0, timeout=2000)
        page.evaluate('window.trackClose=false')
        end = trigger.bounding_box()
        trace = page.evaluate('window.closeFrames')
        (folder/f'{width}-close-frames.json').write_text(json.dumps(trace, indent=2), encoding='utf-8')
        assert all(abs(start[key] - end[key]) < 1 for key in ['x', 'y', 'width', 'height']), (width, start, end)
        near_button = [frame for frame in trace if frame['state'] == 'closed' and frame['panel'] and frame['width'] < 50 and frame['background'] != 'rgba(0, 0, 0, 0)']
        assert all(frame['radius'] > 5 for frame in near_button), (width, 'painted square close frame')
        assert trigger.evaluate('node=>parseFloat(getComputedStyle(node).borderTopLeftRadius)') > 5
        assert trigger.evaluate('node=>getComputedStyle(node).opacity') == '1'
        assert trigger.evaluate('node=>!node.closest("[aria-hidden=true]")')
        (folder/f'{width}-close-frames.json').write_text(json.dumps(trace, indent=2), encoding='utf-8')
        assert page.evaluate('document.documentElement.scrollWidth<=innerWidth')

    # Other menu families use the same light edge, and opening a menu hides hints.
    page.set_viewport_size({'width': 1440, 'height': 1000})
    other = rail.locator('[data-nav=other]')
    other.hover()
    expect(hint).to_be_visible(timeout=2000)
    other.click()
    menu = page.locator('[role=menu][data-spring-tone=menu]')
    expect(menu).to_be_visible()
    page.wait_for_timeout(1000)
    expect(hint).to_have_count(0)
    border(menu.locator('> [data-spring-shell]'), 'other')
    page.keyboard.press('Escape')
    expect(menu).to_have_count(0, timeout=2000)
    approval = page.get_by_role('button', name='请求批准', exact=True)
    approval.click()
    panel = page.locator('.morph-menu-expanded').filter(has=page.locator('[data-menu-item=auto]'))
    expect(panel).to_be_visible()
    page.wait_for_timeout(1000)
    border(panel, 'approval')
    page.keyboard.press('Escape')
    expect(panel).to_have_count(0, timeout=2000)
    assert len(set(result['color'] for result in borders.values())) == 1, borders

    # Reduced motion keeps the content update immediate without moving animation.
    page.emulate_media(reduced_motion='reduce')
    first.hover()
    expect(hint).to_be_visible(timeout=2000)
    next_button.hover()
    expect(hint).to_have_text('搜索', timeout=300)
    page.mouse.move(700, 200)
    expect(hint).to_have_count(0, timeout=1500)
    assert not errors, errors
    print(json.dumps({'result': 'passed', 'frames': len(frames), 'minOpacity': min(f['opacity'] for f in frames),
        'borders': borders, 'checks': ['initial delay only', 'gap continuity', 'same DOM surface and no entrance replay',
        'smooth right-side positioning', 'horizontal toolbar continuity', 'keyboard and Escape', 'menu suppression',
        'settings close keeps round button and position at 375/768/1440', 'shared light borders', 'reduced motion']}, ensure_ascii=False))
    browser.close()
