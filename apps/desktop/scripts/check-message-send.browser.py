"""Run with Vite on 1420: python scripts/check-message-send.browser.py.
Only settings are stubbed; the existing browser run fallback avoids live models.
"""
import json
import os
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

url = os.environ.get('DEEPAGENT_PREVIEW_URL', 'http://127.0.0.1:1420/')
folder = Path(__file__).resolve().parent.parent / '.tmp' / 'message-send-integration'
folder.mkdir(parents=True, exist_ok=True)


def settings(route):
    response = route.fetch()
    source, count = re.subn(
        r'(async function getSettings\(\)\s*\{.*?return )null;',
        r'\1{configured:true,available_models:["deepseek-flash"],model_capabilities:[],chat_model:"deepseek-flash",reasoner_model:"deepseek-flash"};',
        response.text(), count=1, flags=re.S)
    assert count == 1, 'Browser settings boundary changed'
    route.fulfill(response=response, body=source)


with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, channel='chrome')
    page = browser.new_page(viewport={'width': 1440, 'height': 1000})
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.route('**/src/api.ts', settings)
    page.goto(url, wait_until='commit', timeout=60000)
    editor = page.locator('.composer-textarea')
    editor.wait_for(timeout=60000)
    summaries = []

    def send(text, label, click=False, capture=False):
        expect(page.locator('.composer-send-btn--stop')).to_have_count(0, timeout=15000)
        before = page.locator('[data-user-message-id]').count()
        editor.fill(text)
        page.evaluate('''()=>{
          window.sendTrace=[];window.captureSend=true;const started=performance.now();
          const read=()=>{const node=document.querySelector('.message-send-carrier');
            const target=[...document.querySelectorAll('[data-user-message-id]')].at(-1);
            const rect=node?.getBoundingClientRect(),end=target?.getBoundingClientRect();
            window.sendTrace.push({t:performance.now()-started,
              carrier:rect?{x:rect.x,y:rect.y,width:rect.width,height:rect.height}:null,
              end:end?{x:end.x,y:end.y,width:end.width,height:end.height}:null,
              visibility:target?getComputedStyle(target).visibility:null,
              count:document.querySelectorAll('.message-send-carrier').length});
            if(window.captureSend&&performance.now()-started<8000)requestAnimationFrame(read);
          };requestAnimationFrame(read);
        }''')
        if click:
            page.locator('.composer-send-btn').click()
        else:
            editor.press('Enter')
        expect(page.locator('[data-user-message-id]')).to_have_count(before + 1, timeout=15000)
        target = page.locator('[data-user-message-id]').last
        if capture:
            page.wait_for_function("[...document.querySelectorAll('[data-user-message-id]')].at(-1)?.hasAttribute('data-message-send-hidden')", timeout=5000)
            page.wait_for_timeout(220)
            page.screenshot(path=str(folder / f'{label}-in-flight.png'), full_page=True)
        expect(page.locator('.message-send-flight-root')).to_have_count(0, timeout=8000)
        expect(target).to_be_visible()
        assert target.text_content() == text.strip(), label
        page.evaluate('window.captureSend=false')
        trace = page.evaluate('window.sendTrace')
        flying = [frame for frame in trace if frame['carrier'] and frame['visibility'] == 'hidden']
        assert all(frame['count'] <= 1 for frame in trace), label
        duplicated = [frame for frame in trace if frame['carrier'] and frame['visibility'] == 'visible']
        # During lazy mounting the last bubble may be a previous message; once
        # this flight owns its target it must stay hidden through the handoff.
        if flying:
            assert not [frame for frame in duplicated if frame['t'] >= flying[0]['t']], (label, 'duplicate visible message')
        assert len(flying) > 4, (label, 'no actual flight')
        assert abs(flying[-1]['carrier']['y'] - flying[-1]['end']['y']) < 1, (label, 'handoff jumped')
        delta = min(frame['carrier']['y'] - frame['end']['y'] for frame in flying)
        summaries.append({'case': label, 'frames': len(flying), 'overshoot': round(delta, 2)})
        (folder / f'{label}.json').write_text(json.dumps(trace, indent=2), encoding='utf-8')
        return target

    first = send('把这个想法，顺着动作送进消息气泡。', 'first-message', click=True, capture=True)
    assert summaries[-1]['overshoot'] < -1, summaries[-1]
    first_id = first.get_attribute('data-user-message-id')
    send('把这个想法，顺着动作送进消息气泡。', 'same-text-followup', capture=True)
    assert page.locator('[data-user-message-id]').last.get_attribute('data-user-message-id') != first_id
    send('上下文 👋\n整块送进去，再轻轻回弹。', 'multiline-emoji')
    send('长文字保持连贯，不能重复，也不能丢字。' * 14, 'long-text')

    expect(page.locator('.composer-send-btn--stop')).to_have_count(0, timeout=15000)
    before = page.locator('[data-user-message-id]').count()
    editor.fill('正在中文输入')
    editor.dispatch_event('keydown', {'key': 'Enter', 'code': 'Enter', 'isComposing': True, 'bubbles': True})
    assert editor.input_value() == '正在中文输入'
    assert page.locator('[data-user-message-id]').count() == before
    assert page.locator('.message-send-flight-root').count() == 0

    editor.fill('缩放窗口也不留下残影。')
    editor.press('Enter')
    page.wait_for_function("!!document.querySelector('.message-send-carrier')")
    page.set_viewport_size({'width': 768, 'height': 1000})
    expect(page.locator('.message-send-flight-root')).to_have_count(0)
    expect(page.locator('[data-user-message-id]').last).to_be_visible()
    for width in [1440, 768, 375]:
        page.set_viewport_size({'width': width, 'height': 1000})
        send(f'{width} 像素宽度下，消息也要顺畅送达。', f'width-{width}')
        assert page.evaluate('document.documentElement.scrollWidth<=innerWidth'), width
        page.screenshot(path=str(folder / f'{width}-chat.png'), full_page=True)

    expect(page.locator('.composer-send-btn--stop')).to_have_count(0, timeout=15000)
    page.emulate_media(reduced_motion='reduce')
    before = page.locator('[data-user-message-id]').count()
    editor.fill('减少动态效果仍然正常发送。')
    editor.press('Enter')
    expect(page.locator('[data-user-message-id]')).to_have_count(before + 1)
    assert page.locator('.message-send-flight-root').count() == 0
    assert page.locator('[data-user-message-id]').last.evaluate('n=>getComputedStyle(n).visibility') == 'visible'

    expect(page.locator('.composer-send-btn--stop')).to_have_count(0, timeout=15000)
    page.emulate_media(reduced_motion='no-preference')
    editor.fill('切换页面时清理动画。')
    editor.press('Enter')
    page.wait_for_function("!!document.querySelector('.message-send-carrier')")
    page.locator('.app-navigation-home').click()
    expect(page.locator('.message-send-flight-root')).to_have_count(0, timeout=3000)
    assert not errors, errors
    browser.close()
    print(json.dumps({'result': 'passed', 'frontend': url, 'flights': summaries,
        'checks': ['cold start and followup', 'message identity', 'multiline/emoji/long text',
                   'IME Enter', 'resize during flight', '1440/768/375 screenshots',
                   'reduced motion', 'navigation cleanup', 'no page errors']}, ensure_ascii=False))
