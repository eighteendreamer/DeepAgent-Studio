"""Motion regression check. Requires Python Playwright and a running preview."""
import json
from playwright.sync_api import sync_playwright, expect


def capture(page, selector):
    page.evaluate("""() => {
      window.motionFrames=[]; window.motionSampleDone=false;
      const start=performance.now();
      function sample(now) {
        const r=document.querySelector('.share-shell').getBoundingClientRect();
        window.motionFrames.push({time:now-start,width:r.width,height:r.height});
        if(now-start<1200) requestAnimationFrame(sample);
        else window.motionSampleDone=true;
      }
      requestAnimationFrame(sample);
    }""")
    page.locator(selector).click()
    page.wait_for_function('window.motionSampleDone')
    return page.evaluate('window.motionFrames')


def first_change(frames, axis):
    initial=frames[0][axis]
    return next(f['time'] for f in frames if abs(f[axis]-initial)>2)


with sync_playwright() as p:
    browser=p.chromium.launch(headless=True,channel='chrome')
    page=browser.new_page(viewport={'width':768,'height':900})
    errors=[]
    page.on('pageerror',lambda error:errors.append(str(error)))
    page.goto('http://127.0.0.1:5174/')
    page.wait_for_load_state('networkidle')
    page.wait_for_timeout(800)
    closing=capture(page,'.close-button')
    opening=capture(page,'.compact-share')
    collapsed=closing[-1]['height']
    expanded=opening[-1]['height']
    distance=expanded-collapsed
    peak=max(f['height'] for f in opening)
    trough=min(f['height'] for f in closing)
    assert expanded+distance*.04 < peak < expanded+distance*.09,('missing or excessive opening rebound',peak)
    assert collapsed-8 < trough < collapsed-.5,('missing or excessive closing compression',trough)
    assert first_change(opening,'height')-first_change(opening,'width') >= 25,'opening must widen before growing'
    assert first_change(closing,'width')-first_change(closing,'height') >= 25,'closing must flatten before narrowing'
    assert abs(opening[-1]['width']-320)<.2
    expect(page.locator('.share-shell')).to_have_attribute('data-open','true')

    # Interrupt both directions before completion, then verify the final state.
    for _ in range(4):
        page.locator('.demo-signature').click()
        page.wait_for_timeout(85)
    page.wait_for_timeout(1200)
    expect(page.locator('.share-shell')).to_have_attribute('data-open','true')
    size=page.locator('.share-shell').bounding_box()
    assert abs(size['width']-320)<.2 and abs(size['height']-expanded)<.2,size
    assert not page.locator('.share-panel').evaluate('(e)=>e.inert')

    # Hover remains still; pointer press compresses and release returns to 1.
    button=page.locator('.copy-button')
    button.hover()
    page.wait_for_timeout(200)
    assert button.evaluate('(e)=>getComputedStyle(e).transform')=='none'
    page.mouse.down()
    page.wait_for_timeout(110)
    assert button.evaluate('(e)=>new DOMMatrix(getComputedStyle(e).transform).a')<.98
    page.mouse.up()
    page.wait_for_timeout(500)
    assert abs(button.evaluate('(e)=>new DOMMatrix(getComputedStyle(e).transform).a')-1)<.002

    page.emulate_media(reduced_motion='reduce')
    page.locator('.close-button').click()
    expect(page.locator('.share-shell')).to_have_css('height','56px')
    page.locator('.compact-share').click()
    expect(page.locator('.share-shell')).to_have_css('height',f'{expanded:g}px')
    assert not errors,errors
    print(json.dumps({'opening_peak':peak,'opening_rest':expanded,'closing_minimum':trough,
        'closing_rest':collapsed,'opening_height_delay_ms':round(first_change(opening,'height')-first_change(opening,'width')),
        'closing_width_delay_ms':round(first_change(closing,'width')-first_change(closing,'height')),
        'interruption':'passed','pointer_press':'passed','reduced_motion':'passed','page_errors':errors},indent=2))
    browser.close()
