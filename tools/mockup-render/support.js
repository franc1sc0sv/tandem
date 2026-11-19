/*
 * Own minimal runtime for the `mockups/v03/*.dc.html` mockup format (T-125).
 *
 * The original runtime is not in the repo; this one implements what the mockups use:
 * `<x-dc>` with the template, `<helmet>` (goes to `<head>`), `{{expr}}` in text and attributes
 * (including `style` and `onClick="{{fn}}"` handlers), `<sc-for list as>`, `<sc-if value>`
 * and the `Component extends DCLogic` class of the `<script type="text/x-dc">` with `state`,
 * `setState` and `renderVals()`.
 *
 * It is a classic script (not a module) because the mockups load it with `<script src>` in `<head>`.
 */
(function () {
  'use strict';

  var HTML_NS = 'http://www.w3.org/1999/xhtml';
  var EXPR = /\{\{([\s\S]+?)\}\}/g;

  // Diagnostics read by the tests and the capturer (`window.__dc`).
  var dc = (window.__dc = { ready: false, renders: 0, errors: [], missing: [], component: null });

  function report(kind, msg) {
    if (kind === 'error') {
      dc.errors.push(msg);
      console.error('[dc] ' + msg);
    } else {
      if (dc.missing.indexOf(msg) === -1) dc.missing.push(msg);
      console.warn('[dc] ' + msg);
    }
  }

  // ---- Expressions -------------------------------------------------------------------------

  var compiled = new Map();

  // `with` resolves names against the scope (values of `renderVals()` and variables of
  // `sc-for`); whatever is not there falls to the globals (`Math`, `true`…). Functions created
  // with `Function` do not inherit strict mode, so `with` is valid here.
  function compile(src) {
    var fn = compiled.get(src);
    if (!fn) {
      fn = new Function('__scope', 'with (__scope) { return (' + src + '\n); }');
      compiled.set(src, fn);
    }
    return fn;
  }

  function evaluate(src, scope) {
    var value;
    try {
      value = compile(src)(scope);
    } catch (e) {
      report('error', '{{' + src.trim() + '}}: ' + (e && e.message ? e.message : String(e)));
      return undefined;
    }
    if (value === undefined) report('missing', '{{' + src.trim() + '}} is undefined');
    return value;
  }

  // Splits a text into literals and expressions.
  function tokens(str) {
    var out = [];
    var last = 0;
    var m;
    EXPR.lastIndex = 0;
    while ((m = EXPR.exec(str))) {
      if (m.index > last) out.push({ text: str.slice(last, m.index) });
      out.push({ expr: m[1] });
      last = m.index + m[0].length;
    }
    if (last < str.length) out.push({ text: str.slice(last) });
    return out;
  }

  function toText(v) {
    return v === null || v === undefined || typeof v === 'boolean' ? '' : String(v);
  }

  // If the text is a single expression (with at most surrounding spaces), returns the raw
  // value (function, object, boolean); otherwise the text with each expression substituted.
  function evalTemplate(str, scope) {
    var parts = tokens(str);
    var exprs = parts.filter(function (p) { return 'expr' in p; });
    var onlyBlank = parts.every(function (p) { return 'expr' in p || !p.text.trim(); });
    if (exprs.length === 1 && onlyBlank) return evaluate(exprs[0].expr, scope);
    return parts.map(function (p) { return 'expr' in p ? toText(evaluate(p.expr, scope)) : p.text; }).join('');
  }

  function child(scope, name, value) {
    var s = Object.create(scope);
    s[name] = value;
    return s;
  }

  // ---- Attributes --------------------------------------------------------------------------

  function kebab(name) {
    return name.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); });
  }

  // Attribute value as a string, or null if the attribute must not be present.
  function attrValue(name, v) {
    if (v === null || v === undefined || typeof v === 'function') return null;
    var aria = name.indexOf('aria-') === 0 || name.indexOf('data-') === 0;
    if (typeof v === 'boolean') return aria ? String(v) : v ? '' : null;
    if (name === 'style' && typeof v === 'object') {
      return Object.keys(v).map(function (k) { return kebab(k) + ':' + v[k]; }).join(';');
    }
    if (name === 'class' && Array.isArray(v)) return v.filter(Boolean).join(' ');
    return String(v);
  }

  var EVENT_ALIAS = { doubleclick: 'dblclick' };

  function bind(el, handlers) {
    el.__dcOn = handlers;
    if (!el.__dcTypes) el.__dcTypes = new Set();
    Object.keys(handlers).forEach(function (type) {
      if (el.__dcTypes.has(type)) return;
      el.__dcTypes.add(type);
      el.addEventListener(type, function (e) {
        var h = el.__dcOn && el.__dcOn[type];
        if (h) h(e);
      });
    });
  }

  // ---- Render ------------------------------------------------------------------------------

  function renderChildren(tpl, scope, out) {
    var nodes = tpl.childNodes;
    for (var i = 0; i < nodes.length; i++) renderNode(nodes[i], scope, out);
  }

  function renderNode(n, scope, out) {
    if (n.nodeType === 3) {
      var t = n.nodeValue;
      out.appendChild(document.createTextNode(t.indexOf('{{') === -1 ? t : toText(evalTemplate(t, scope))));
      return;
    }
    if (n.nodeType !== 1) return;
    var tag = n.localName;

    if (tag === 'helmet') return;
    if (tag === 'sc-for') {
      var list = evalTemplate(n.getAttribute('list') || '', scope);
      var as = n.getAttribute('as') || 'item';
      var index = n.getAttribute('index');
      var items = [];
      if (Array.isArray(list)) items = list;
      else if (list && typeof list[Symbol.iterator] === 'function') items = Array.from(list);
      else if (list !== null && list !== undefined) report('error', '<sc-for list="' + n.getAttribute('list') + '"> is not a list');
      items.forEach(function (item, i) {
        var s = child(scope, as, item);
        if (index) s = child(s, index, i);
        renderChildren(n, s, out);
      });
      return;
    }
    if (tag === 'sc-if') {
      if (evalTemplate(n.getAttribute('value') || '', scope)) renderChildren(n, scope, out);
      return;
    }
    if (tag === 'script') {
      report('missing', '<script> inside the template: ignored');
      return;
    }

    var el = n.namespaceURI === HTML_NS
      ? document.createElement(tag)
      : document.createElementNS(n.namespaceURI, n.localName);
    var handlers = {};
    for (var i = 0; i < n.attributes.length; i++) {
      var a = n.attributes[i];
      var name = a.name;
      if (name.indexOf('hint-placeholder') === 0) continue;
      if (a.value.indexOf('{{') === -1) {
        if (a.namespaceURI) el.setAttributeNS(a.namespaceURI, name, a.value);
        else el.setAttribute(name, a.value);
        continue;
      }
      var v = evalTemplate(a.value, scope);
      if (name.indexOf('on') === 0 && name.length > 2) {
        var type = name.slice(2).toLowerCase();
        if (typeof v === 'function') handlers[EVENT_ALIAS[type] || type] = v;
        else if (v !== undefined) report('error', name + '="' + a.value + '" is not a function');
        continue;
      }
      var s = attrValue(name, v);
      if (s !== null) el.setAttribute(name, s);
    }
    bind(el, handlers);

    if (tag === 'style') el.textContent = n.textContent; // CSS as is: no substitution
    else renderChildren(n, scope, el);
    if ((tag === 'input' || tag === 'textarea') && el.hasAttribute('value')) el.value = el.getAttribute('value');
    out.appendChild(el);
  }

  // ---- Reconciliation ----------------------------------------------------------------------
  // The existing DOM is patched instead of replaced, so CSS transitions run
  // (for example, the `V3-Arrangement` screens slide) and an `<input>` does not lose focus.

  function same(a, b) {
    return a.nodeType === b.nodeType && (a.nodeType !== 1 || (a.localName === b.localName && a.namespaceURI === b.namespaceURI));
  }

  function morphChildren(cur, next) {
    var a = Array.prototype.slice.call(cur.childNodes);
    var b = Array.prototype.slice.call(next.childNodes);
    for (var i = 0; i < b.length; i++) {
      if (!a[i]) cur.appendChild(b[i]);
      else if (same(a[i], b[i])) morph(a[i], b[i]);
      else cur.replaceChild(b[i], a[i]);
    }
    for (var j = b.length; j < a.length; j++) cur.removeChild(a[j]);
  }

  function morph(o, n) {
    if (o.nodeType !== 1) {
      if (o.nodeValue !== n.nodeValue) o.nodeValue = n.nodeValue;
      return;
    }
    var i;
    for (i = o.attributes.length - 1; i >= 0; i--) {
      var oa = o.attributes[i];
      if (!n.hasAttributeNS(oa.namespaceURI, oa.localName)) o.removeAttributeNS(oa.namespaceURI, oa.localName);
    }
    for (i = 0; i < n.attributes.length; i++) {
      var na = n.attributes[i];
      if (o.getAttributeNS(na.namespaceURI, na.localName) !== na.value) {
        if (na.namespaceURI) o.setAttributeNS(na.namespaceURI, na.name, na.value);
        else o.setAttribute(na.name, na.value);
      }
    }
    bind(o, n.__dcOn || {});
    if ((o.localName === 'input' || o.localName === 'textarea') && n.hasAttribute('value') && o.value !== n.value) {
      o.value = n.value;
    }
    if (o.localName === 'style') {
      if (o.textContent !== n.textContent) o.textContent = n.textContent;
      return;
    }
    morphChildren(o, n);
  }

  // ---- Component ---------------------------------------------------------------------------

  function DCLogic(props) {
    this.props = props || {};
    this.state = {};
  }
  DCLogic.prototype.setState = function (update, callback) {
    var patch = typeof update === 'function' ? update(this.state, this.props) : update;
    if (patch !== null && patch !== undefined) this.state = Object.assign({}, this.state, patch);
    if (this.__dcSchedule) this.__dcSchedule(callback);
  };
  DCLogic.prototype.renderVals = function () {
    return {};
  };
  window.DCLogic = DCLogic;

  // ---- Startup -----------------------------------------------------------------------------
  // The template HTML cannot go through the page parser: attributes such as
  // `d="{{p.ic}}"` on a `<path>` already give console errors when parsed. So, if this
  // script is loaded from the parser (the usual case: `<script src>` in `<head>`), it turns the rest
  // of the document into text with `<plaintext>` and parses it separately with DOMParser, which is inert.

  var fromParser = document.readyState === 'loading' && !!document.currentScript;
  if (fromParser) document.write('<plaintext hidden data-dc-source>');

  function source() {
    var pt = document.querySelector('plaintext[data-dc-source]');
    if (pt) {
      var raw = pt.textContent;
      pt.remove();
      return new DOMParser().parseFromString(raw, 'text/html');
    }
    // Loaded late (without `<plaintext>`): the template is already on the page; it is moved to an
    // inert document to render from there.
    var inert = document.implementation.createHTMLDocument('');
    var nodes = document.querySelectorAll('x-dc, script[type="text/x-dc"]');
    for (var i = 0; i < nodes.length; i++) inert.body.appendChild(inert.adoptNode(nodes[i]));
    return inert;
  }

  function start() {
    var src = source();
    var tpl = src.querySelector('x-dc');
    if (!tpl) {
      report('error', 'there is no <x-dc> on the page');
      return;
    }

    var base = document.createElement('style');
    base.textContent = 'x-dc{display:contents}';
    document.head.appendChild(base);
    var helmets = tpl.querySelectorAll('helmet');
    for (var h = 0; h < helmets.length; h++) {
      var kids = helmets[h].childNodes;
      for (var k = 0; k < kids.length; k++) {
        if (kids[k].nodeType === 1) document.head.appendChild(document.importNode(kids[k], true));
      }
    }
    var bodyAttrs = src.body ? src.body.attributes : [];
    for (var b = 0; b < bodyAttrs.length; b++) document.body.setAttribute(bodyAttrs[b].name, bodyAttrs[b].value);

    var host = document.createElement('x-dc');
    document.body.appendChild(host);

    var script = src.querySelector('script[type="text/x-dc"]');
    var props = {};
    var comp;
    try {
      if (script && script.getAttribute('data-props')) props = JSON.parse(script.getAttribute('data-props'));
      var Comp = script
        ? new Function('DCLogic', script.textContent + '\n;return typeof Component === "undefined" ? undefined : Component;')(DCLogic)
        : undefined;
      comp = Comp ? new Comp(props) : new DCLogic(props);
    } catch (e) {
      report('error', 'the component does not start: ' + (e && e.message ? e.message : String(e)));
      comp = new DCLogic(props);
    }
    dc.component = comp;

    var mounted = false;
    var pending = null;

    function render() {
      var vals;
      try {
        vals = comp.renderVals() || {};
      } catch (e) {
        report('error', 'renderVals(): ' + (e && e.message ? e.message : String(e)));
        return;
      }
      var scope = Object.create(null);
      scope.state = comp.state;
      scope.props = comp.props;
      Object.keys(vals).forEach(function (key) { scope[key] = vals[key]; });
      var frag = document.createDocumentFragment();
      renderChildren(tpl, scope, frag);
      if (mounted) morphChildren(host, frag);
      else host.appendChild(frag);
      mounted = true;
      dc.renders += 1;
    }

    // Several consecutive `setState` calls (in the same handler) give a single render.
    comp.__dcSchedule = function (callback) {
      if (!pending) {
        pending = [];
        queueMicrotask(function () {
          var callbacks = pending;
          pending = null;
          render();
          callbacks.forEach(function (cb) { cb(); });
        });
      }
      if (typeof callback === 'function') pending.push(callback);
    };

    render();
    if (typeof comp.componentDidMount === 'function') comp.componentDidMount();
    requestAnimationFrame(function () {
      dc.ready = true;
      document.documentElement.setAttribute('data-dc-ready', '');
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();
