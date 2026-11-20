function workspaceBrowser(request) {
  try {
    const scope = 'http:' === location.protocol || 'https:' === location.protocol;
    if (!scope || window.top !== window) return { error: 'This page cannot be controlled' };
    const status = () => ({ url: location.href, title: document.title, state: document.readyState });
    if (request.action === 'snapshot') {
      const visible = el => el.getClientRects().length && getComputedStyle(el).visibility === 'visible';
      const targets = new Map();
      const seed = Math.random().toString(36).slice(2, 10);
      const elements = [];
      for (const el of document.querySelectorAll('a[href],button,input:not([type=hidden]),textarea,[role=button]')) {
        if (!visible(el) || el.disabled || el.closest('[inert]')) continue;
        const ref = `${seed}:${targets.size + 1}`;
        targets.set(ref, el);
        const label = el.getAttribute('aria-label') || el.labels?.[0]?.textContent || el.innerText || el.getAttribute('placeholder') || '';
        elements.push({ ref, kind: el.localName, label: label.trim().slice(0, 160) });
        if (targets.size === 64) break;
      }
      Object.defineProperty(document, '__deepagentBrowserRefs', { configurable: true, value: { url: location.href, targets } });
      return { ...status(), action: request.action, text: (document.body?.innerText || '').slice(0, 9000), elements };
    }
    if (request.action === 'scroll') {
      window.scrollBy({ top: request.delta_y, behavior: 'instant' });
      return { ...status(), action: request.action, scroll_y: window.scrollY };
    }
    const refs = document.__deepagentBrowserRefs;
    const element = refs?.url === location.href && refs.targets.get(request.target);
    if (!element?.isConnected || !element.getClientRects().length || element.disabled || element.closest('[inert]')) {
      return { error: 'Element reference expired; take a new snapshot' };
    }
    if (request.action === 'click') {
      element.scrollIntoView({ block: 'nearest' });
      element.click();
    } else if (request.action === 'fill') {
      const type = element.getAttribute('type')?.toLowerCase() || 'text';
      const editable = element instanceof HTMLTextAreaElement ||
        (element instanceof HTMLInputElement && ['text','search','email','url','tel','number'].includes(type));
      if (!editable || element.readOnly || (element.autocomplete || '').includes('one-time-code')) {
        return { error: 'This input cannot be filled by the browser tool' };
      }
      element.focus();
      const proto = element instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(element, request.text);
      element.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
      element.dispatchEvent(new Event('change', { bubbles: true }));
    }
    return { ...status(), action: request.action, target: request.target };
  } catch {
    return { error: 'Browser action could not finish on this document; take a new snapshot' };
  }
}
