// Tests of our own support.js in Chromium, with minimal pages of the same format.
import type { Browser, Page } from 'playwright';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ORIGIN, checkPage, installRoutes, launchBrowser, waitForMockup } from '../src/browser.ts';

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
});
afterAll(async () => {
  await browser?.close();
});

function doc(body: string, component = '', props = '{"$preview":{"width":800,"height":600}}'): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>prueba</title>
<script src="./support.js"></script>
</head>
<body>
<x-dc>
${body}
</x-dc>
${component ? `<script type="text/x-dc" data-dc-script data-props='${props}'>\n${component}\n</script>` : ''}
</body>
</html>`;
}

interface Opened {
  page: Page;
  consoleErrors: string[];
}

async function open(pages: Record<string, string>, path = Object.keys(pages)[0] ?? '/'): Promise<Opened> {
  const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
  const consoleErrors: string[] = [];
  await installRoutes(context, { pages, onBlocked: (u) => consoleErrors.push('bloqueada: ' + u) });
  const page = await context.newPage();
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text());
  });
  page.on('pageerror', (e) => consoleErrors.push(e.message));
  await page.goto(ORIGIN + path);
  await waitForMockup(page);
  return { page, consoleErrors };
}

const text = (page: Page, sel: string) => page.locator(sel).first().innerText();
const attr = (page: Page, sel: string, name: string) => page.locator(sel).first().getAttribute(name);

describe('support.js', () => {
  it('sustituye {{expr}} en texto y atributos, incluidos style, SVG y expresiones', async () => {
    const { page, consoleErrors } = await open({
      '/a.dc.html': doc(
        `<p id="t">Hola {{name}}, tienes {{ n + 1 }} avisos{{ none }}</p>
         <p id="s" style="margin:0;{{st}}" title="{{ n > 1 ? 'muchos' : 'uno' }}" data-x="{{ true }}">x</p>
         <svg width="10" height="10"><path id="p" d="{{ic}}" stroke="{{c}}"></path><stop id="g" stop-color="{{c}}"></stop></svg>
         <button id="b" aria-pressed="{{on}}" class="{{cls}}" disabled="{{ false }}">b</button>`,
        `class Component extends DCLogic {
          renderVals() { return { name: 'Ana', n: 2, none: null, st: 'color:rgb(255, 0, 0)', ic: 'M0 0h10', c: '#7aa7e6', on: 'true', cls: 'btn pri' }; }
        }`,
      ),
    });
    expect(await text(page, '#t')).toBe('Hola Ana, tienes 3 avisos');
    expect(await attr(page, '#s', 'style')).toBe('margin:0;color:rgb(255, 0, 0)');
    expect(await page.locator('#s').evaluate((e) => getComputedStyle(e).color)).toBe('rgb(255, 0, 0)');
    expect(await attr(page, '#s', 'title')).toBe('muchos');
    expect(await attr(page, '#p', 'd')).toBe('M0 0h10');
    expect(await attr(page, '#p', 'stroke')).toBe('#7aa7e6');
    expect(await page.locator('#p').evaluate((e) => e.namespaceURI)).toBe('http://www.w3.org/2000/svg');
    expect(await attr(page, '#g', 'stop-color')).toBe('#7aa7e6');
    expect(await attr(page, '#b', 'aria-pressed')).toBe('true');
    expect(await attr(page, '#b', 'class')).toBe('btn pri');
    expect(await attr(page, '#b', 'disabled')).toBeNull();
    // A `d="{{…}}"` parsed by the page would give a console error: there must be none.
    expect(consoleErrors).toEqual([]);
    const check = await checkPage(page);
    expect(check.braces).toEqual([]);
    expect(check.dcErrors).toEqual([]);
  });

  it('repeats sc-for (nested, with access to the outer scope) and shows or hides sc-if', async () => {
    const { page, consoleErrors } = await open({
      '/a.dc.html': doc(
        `<ul><sc-for list="{{groups}}" as="g" hint-placeholder-count="3">
           <li class="g">{{g.name}}<sc-for list="{{g.items}}" as="i"><span class="i">{{title}}/{{g.name}}/{{i}}</span></sc-for></li>
         </sc-for></ul>
         <sc-if value="{{yes}}" hint-placeholder-val="{{ false }}"><b id="yes">yes</b></sc-if>
         <sc-if value="{{no}}" hint-placeholder-val="{{ true }}"><b id="no">no</b></sc-if>
         <sc-for list="{{empty}}" as="x"><i class="x">{{x}}</i></sc-for>`,
        `class Component extends DCLogic {
          renderVals() { return { title: 'T', yes: true, no: false, empty: [],
            groups: [{ name: 'a', items: [1, 2] }, { name: 'b', items: [3] }] }; }
        }`,
      ),
    });
    expect(await page.locator('li.g').count()).toBe(2);
    expect(await page.locator('span.i').allInnerTexts()).toEqual(['T/a/1', 'T/a/2', 'T/b/3']);
    expect(await page.locator('#yes').count()).toBe(1);
    expect(await page.locator('#no').count()).toBe(0);
    expect(await page.locator('.x').count()).toBe(0);
    // Template tags do not stay on the page (they do not break flex or grid).
    expect(await page.locator('sc-for, sc-if').count()).toBe(0);
    expect(consoleErrors).toEqual([]);
  });

  it('puts the content of <helmet> in <head>', async () => {
    const { page } = await open({
      '/a.dc.html': doc(
        `<helmet><style>.k{color:rgb(1, 2, 3)}</style></helmet><p class="k" id="k">k</p>`,
        `class Component extends DCLogic { renderVals() { return {}; } }`,
      ),
    });
    const headCss = await page.evaluate(() => Array.from(document.head.querySelectorAll('style')).map((s) => s.textContent));
    expect(headCss).toContain('.k{color:rgb(1, 2, 3)}');
    expect(await page.locator('helmet').count()).toBe(0);
    expect(await page.locator('#k').evaluate((e) => getComputedStyle(e).color)).toBe('rgb(1, 2, 3)');
  });

  it('constructor(p) receives data-props; onClick calls the handler and setState re-renders by patching the DOM', async () => {
    const { page, consoleErrors } = await open({
      '/a.dc.html': doc(
        `<p id="w">{{w}}</p>
         <sc-for list="{{items}}" as="it"><button class="it" onClick="{{it.pick}}" style="{{it.style}}">{{it.name}}</button></sc-for>
         <button id="inc" onClick="{{inc}}">+</button><span id="n">{{n}}</span>`,
        `class Component extends DCLogic {
          constructor(p) { super(p); this.state = { sel: 'a', n: 0 }; }
          renderVals() {
            return {
              w: this.props.$preview.width,
              n: this.state.n,
              inc: () => { this.setState({ n: this.state.n + 1 }); this.setState((s) => ({ n: s.n + 1 })); },
              items: ['a', 'b'].map((id) => ({ name: id, style: id === this.state.sel ? 'color:rgb(0, 128, 0)' : '', pick: () => this.setState({ sel: id }) })),
            };
          }
        }`,
      ),
    });
    expect(await text(page, '#w')).toBe('800');
    // Mark on the node: if the render replaced the DOM, the mark would be lost.
    await page.locator('.it').nth(1).evaluate((e) => ((e as HTMLElement & { mark?: number }).mark = 1));
    await page.locator('.it').nth(1).click();
    await expect.poll(() => attr(page, '.it >> nth=1', 'style')).toBe('color:rgb(0, 128, 0)');
    expect(await attr(page, '.it >> nth=0', 'style')).toBe('');
    expect(await page.locator('.it').nth(1).evaluate((e) => (e as HTMLElement & { mark?: number }).mark)).toBe(1);
    await page.locator('#inc').click();
    await expect.poll(() => text(page, '#n')).toBe('2');
    expect(await page.evaluate(() => (window as unknown as { __dc: { renders: number } }).__dc.renders)).toBe(3);
    expect(consoleErrors).toEqual([]);
  });

  it('an <input> with value/onInput keeps the focus and what was typed', async () => {
    const { page } = await open({
      '/a.dc.html': doc(
        `<input id="q" value="{{q}}" onInput="{{type}}"><p id="echo">{{q}}</p>`,
        `class Component extends DCLogic {
          constructor(p) { super(p); this.state = { q: '' }; }
          renderVals() { return { q: this.state.q, type: (e) => this.setState({ q: e.target.value }) }; }
        }`,
      ),
    });
    await page.locator('#q').pressSequentially('hola');
    await expect.poll(() => text(page, '#echo')).toBe('hola');
    expect(await page.locator('#q').inputValue()).toBe('hola');
    expect(await page.evaluate(() => document.activeElement?.id)).toBe('q');
  });

  it('X.dc.html links navigate to another mockup', async () => {
    const { page, consoleErrors } = await open({
      '/a.dc.html': doc(`<a id="go" href="b.dc.html">ir</a>`),
      '/b.dc.html': doc(`<h1 id="b">{{title}}</h1>`, `class Component extends DCLogic { renderVals() { return { title: 'Page B' }; } }`),
    });
    await page.locator('#go').click();
    await page.waitForURL(/b\.dc\.html$/);
    await waitForMockup(page);
    expect(await text(page, '#b')).toBe('Page B');
    expect(consoleErrors).toEqual([]);
  });

  it('a page without a component renders as is', async () => {
    const { page } = await open({ '/a.dc.html': doc(`<p id="p">static</p>`) });
    expect(await text(page, '#p')).toBe('static');
  });

  it('reports the expressions that fail and those that give undefined', async () => {
    const { page, consoleErrors } = await open({
      '/a.dc.html': doc(
        `<p>{{ nobody.name }}</p><p>{{ obj.missing }}</p>`,
        `class Component extends DCLogic { renderVals() { return { obj: {} }; } }`,
      ),
    });
    const check = await checkPage(page);
    expect(check.dcErrors.join('\n')).toMatch(/nobody\.name/);
    expect(check.dcMissing.join('\n')).toMatch(/obj\.missing/);
    expect(consoleErrors.join('\n')).toMatch(/nobody\.name/);
  });
});
