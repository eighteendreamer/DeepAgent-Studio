"""Browser regression for button-to-popup morphs and directional navigation.

Run against Vite. Only the browser getSettings response is stubbed; no native
settings, credentials, permissions, balances or account data are modified.
"""
import argparse
import json
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', default='http://127.0.0.1:1421/')
parser.add_argument('--built', action='store_true', help='Run against the production build instead of Vite')
parser.add_argument('--output', type=Path, default=Path(__file__).resolve().parents[1]/'.tmp/popup-morph-regression')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)

def settings(route):
    response = route.fetch()
    fixture='{configured:true,available_models:["deepseek-chat"],model_capabilities:[],chat_model:"deepseek-chat",reasoner_model:"deepseek-reasoner"}'
    if args.built:
        source,count=re.subn(r'(async function [\w$]+\(\)\{const ([\w$]+)=[\w$]+\(\);return \2\?\2\("get_settings"\):)null(\})',r'\1'+fixture+r'\3',response.text(),count=1)
        if '"get_settings"' in response.text(): assert count==1,'Compiled settings boundary changed'
    else:
        source,count=re.subn(r'(async function getSettings\(\)\s*\{.*?return )null;',r'\1'+fixture+';',response.text(),count=1,flags=re.S)
        assert count==1
    route.fulfill(response=response, body=source)

def sample(page):
    page.evaluate('''() => {
      window.popupFrames=[]; window.popupDone=false; const start=performance.now();
      function tick(now) {
        window.popupFrames.push([...document.querySelectorAll('[data-spring-shell]')]
          .filter(n=>n.offsetWidth && getComputedStyle(n).visibility!=='hidden' && getComputedStyle(n.parentElement).visibility!=='hidden')
          .map(n=>{const r=n.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,bottom:r.bottom,
            color:getComputedStyle(n).backgroundColor,opacity:getComputedStyle(n).opacity};}));
        if(now-start<1100)requestAnimationFrame(tick);else window.popupDone=true;
      } requestAnimationFrame(tick);
    }''')

def frames(page):
    page.wait_for_function('window.popupDone')
    result = [n for frame in page.evaluate('window.popupFrames') for n in frame]
    for frame in result:
        assert re.match(r'^rgba?\(',frame['color']), ('Unsupported theme color reached GSAP',frame)
        channels=[float(v) for v in re.findall(r'[\d.]+',frame['color'])]
        assert not (channels[0]>180 and channels[1]>180 and channels[2]<100), ('Yellow flash',frame)
    return result

def restored(trigger, origin):
    assert trigger.evaluate('n=>getComputedStyle(n).opacity') == '1'
    returned=trigger.bounding_box()
    assert all(abs(returned[key]-origin[key])<1 for key in ['x','y','width','height']), (origin,returned)

with sync_playwright() as p:
    browser=p.chromium.launch(headless=True,channel='chrome')
    page=browser.new_page(viewport={'width':1440,'height':1000})
    errors=[]
    page.on('pageerror',lambda error:errors.append(str(error)))
    page.route('**/assets/main-*.js' if args.built else '**/src/api.ts',settings)
    page.goto(args.url,wait_until='domcontentloaded',timeout=60000)
    page.get_by_role('heading',name='我们应该做些什么？').wait_for(timeout=60000)
    page.wait_for_load_state('networkidle')
    context=page.get_by_role('button',name='Context 0%',exact=True)
    origin=context.bounding_box()
    sample(page);context.hover()
    opening=frames(page)
    (args.output/'opening-frames.json').write_text(json.dumps(opening),encoding='utf8')
    shell=page.locator('[data-slot="hover-card-content"] > [data-spring-shell]')
    expect(shell).to_be_visible()
    card=shell.bounding_box()
    assert abs(card['y']+card['height']-origin['y']-origin['height'])<1
    assert all(abs(f['bottom']-origin['y']-origin['height'])<1 for f in opening), (origin,max(opening,key=lambda f:abs(f['bottom']-origin['y']-origin['height'])))
    assert max(f['height'] for f in opening)>card['height']+1, 'Reference rebound disappeared'
    assert context.evaluate('n=>getComputedStyle(n).opacity')=='0', 'A second stationary trigger is visible'
    expect(shell.get_by_text('预计下一次请求',exact=True)).to_be_visible()
    expect(shell.get_by_role('button')).to_have_count(0)
    page.mouse.move(card['x']+card['width']/2,card['y']+12)
    page.wait_for_timeout(250)
    expect(shell).to_be_visible()
    page.screenshot(path=str(args.output/'context-expanded.png'))
    sample(page);page.keyboard.press('Escape');page.mouse.move(1300,100)
    closing=frames(page)
    expect(shell).to_have_count(0)
    assert all(abs(f['bottom']-origin['y']-origin['height'])<1 for f in closing)
    assert closing[-1]['width']<=origin['width']+1, 'The full popup flashed again during unmount'
    restored(context,origin)
    # Hover away closes; quick return during close reverses without two surfaces.
    context.hover();expect(shell).to_be_visible();page.wait_for_timeout(1000)
    page.mouse.move(1300,100);page.wait_for_timeout(220);context.hover();page.wait_for_timeout(1000)
    expect(shell).to_have_count(1)
    page.mouse.move(1300,100);expect(shell).to_have_count(0)
    restored(context,origin)
    # Click and keyboard access remain available, including Escape focus return.
    context.focus();page.wait_for_timeout(1050)
    expect(shell).to_be_visible();page.keyboard.press('Escape');expect(shell).to_have_count(0)
    expect(context).to_be_focused()
    page.keyboard.press('Enter');expect(shell).to_be_visible();page.keyboard.press('Escape');expect(shell).to_have_count(0)
    # Left-bottom settings expands over its own original slot, without a header.
    settings_button=page.get_by_role('button',name='设置',exact=True)
    settings_origin=settings_button.bounding_box()
    sample(page);settings_button.click();frames(page)
    settings_shell=page.locator('.spring-surface[role="menu"] > [data-spring-shell]')
    box=settings_shell.bounding_box()
    assert abs(box['x']-settings_origin['x'])<1
    assert abs(box['y']+box['height']-settings_origin['y']-settings_origin['height'])<1
    assert settings_button.evaluate('n=>{let a=1;for(let p=n;p;p=p.parentElement)a*=parseFloat(getComputedStyle(p).opacity);return a;}')==0
    expect(settings_shell.locator('.spring-surface-content').get_by_text('设置',exact=True)).to_be_visible()
    expect(settings_shell.get_by_role('button')).to_have_count(0)
    page.screenshot(path=str(args.output/'settings-expanded.png'))
    page.keyboard.press('Escape');expect(settings_shell).to_have_count(0);restored(settings_button,settings_origin)
    # The Other navigation is intentionally a right flyout, with its row retained.
    other=page.get_by_role('button',name='其他',exact=True)
    other_origin=other.bounding_box();other.hover();page.wait_for_timeout(1100)
    right=page.locator('.spring-surface[data-side="right"] > [data-spring-shell]')
    expect(right).to_have_count(1)
    box=right.bounding_box()
    assert box['x']>=other_origin['x']+other_origin['width'], (other_origin,box)
    assert abs(box['y']-other_origin['y'])<2, (other_origin,box)
    restored(other,other_origin)
    page.mouse.move(box['x']+20,box['y']+15);page.wait_for_timeout(250);expect(right).to_be_visible()
    page.screenshot(path=str(args.output/'other-right.png'))
    page.keyboard.press('Escape');page.mouse.move(1300,100);expect(right).to_have_count(0)
    # The project header's tooltip-wrapped More button must still open its menu.
    project_header=page.get_by_text('项目',exact=True).locator('xpath=..')
    project_header.hover();more=project_header.locator('button').first;more.click();page.wait_for_timeout(1100)
    header_menu=page.locator('.spring-surface[data-state="open"] > [data-spring-shell]')
    expect(header_menu).to_have_count(1)
    assert page.get_by_role('menuitem').count()>2
    page.keyboard.press('Escape');expect(header_menu).to_have_count(0)
    # No stale trigger opacity or orphan surfaces after viewport/reduced-motion changes.
    page.keyboard.press('Control+b');page.wait_for_timeout(550)
    for width in [375,768,1440]:
        page.set_viewport_size({'width':width,'height':1000});page.wait_for_timeout(350)
        context.hover();page.wait_for_timeout(1100)
        box=shell.bounding_box()
        assert box['x']>=7 and box['x']+box['width']<=width-7 and box['y']>=7,(width,box)
        page.screenshot(path=str(args.output/f'context-{width}.png'))
        page.keyboard.press('Escape');page.mouse.move(width-10,100);expect(shell).to_have_count(0)
    page.emulate_media(reduced_motion='reduce')
    context.click();expect(shell).to_be_visible();page.keyboard.press('Escape');expect(shell).to_have_count(0)
    assert context.evaluate('n=>getComputedStyle(n).opacity')=='1'
    assert not errors,errors
    result={'context':'hover/click/keyboard/reversal/one shell passed','settings':'origin and original content passed',
      'other':'right flyout and retained navigation passed','colors':'no yellow frames','viewports':[375,768,1440],'reducedMotion':'passed','pageErrors':errors}
    (args.output/'results.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
    print(json.dumps(result,ensure_ascii=False))
    browser.close()
