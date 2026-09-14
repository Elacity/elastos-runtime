// Test-only stand-in for the two browser pieces the book reader needs:
// a markup parser and a document that can make nodes. It is a deliberate
// approximation - it reproduces tag nesting, attributes, comments, CDATA and
// raw-text elements, and nothing else. The reader itself always uses the real
// browser APIs. Not loaded by the reader.

export const ELEMENT_NODE = 1;
export const TEXT_NODE = 3;
export const CDATA_NODE = 4;
export const COMMENT_NODE = 8;
export const DOCUMENT_NODE = 9;
export const FRAGMENT_NODE = 11;

const VOID_TAGS = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);
const RAW_TEXT_TAGS = new Set(["script", "style"]);
const NAMESPACES = new Map([
  ["xlink", "http://www.w3.org/1999/xlink"],
  ["xml", "http://www.w3.org/XML/1998/namespace"],
  ["epub", "http://www.idpf.org/2007/ops"],
]);

function decodeEntities(value) {
  return value.replace(/&(#x[0-9a-fA-F]+|#\d+|[a-zA-Z]+);/g, (match, body) => {
    if (body.startsWith("#x") || body.startsWith("#X")) {
      return String.fromCodePoint(Number.parseInt(body.slice(2), 16));
    }
    if (body.startsWith("#")) return String.fromCodePoint(Number.parseInt(body.slice(1), 10));
    const named = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };
    return Object.hasOwn(named, body) ? named[body] : match;
  });
}

function makeAttribute(rawName, rawValue) {
  const colon = rawName.indexOf(":");
  const prefix = colon > 0 ? rawName.slice(0, colon) : "";
  return {
    name: rawName,
    localName: colon > 0 ? rawName.slice(colon + 1) : rawName,
    namespaceURI: NAMESPACES.get(prefix) || (prefix ? `urn:test:${prefix}` : null),
    value: decodeEntities(rawValue),
  };
}

function makeElement(tagName, attributes = []) {
  const element = {
    nodeType: ELEMENT_NODE,
    nodeName: tagName.toUpperCase(),
    tagName: tagName.toUpperCase(),
    localName: tagName.toLowerCase(),
    attributes,
    childNodes: [],
    getAttribute(name) {
      const found = element.attributes.find((attribute) => attribute.name === name);
      return found ? found.value : null;
    },
    setAttribute(name, value) {
      const found = element.attributes.find((attribute) => attribute.name === name);
      if (found) {
        found.value = String(value);
        return;
      }
      element.attributes.push(makeAttribute(String(name), ""));
      element.attributes[element.attributes.length - 1].value = String(value);
    },
    appendChild(child) {
      element.childNodes.push(child);
      return child;
    },
  };
  return element;
}

function makeText(data, nodeType = TEXT_NODE) {
  return { nodeType, data, nodeValue: data, childNodes: [] };
}

// A quoted attribute value may hold a ">", so the end of an open tag is found
// with the quote state in hand rather than by looking for the first ">".
function findTagEnd(source, open) {
  let quote = "";
  let afterEquals = false;
  for (let index = open + 1; index < source.length; index += 1) {
    const character = source[index];
    if (quote) {
      if (character === quote) quote = "";
      continue;
    }
    if (character === ">") return index;
    if (character === "=") {
      afterEquals = true;
      continue;
    }
    if (/\s/.test(character)) continue;
    // A quote only opens a value straight after an "=", the way a real parser
    // reads one; a stray quote elsewhere is just part of an attribute name.
    if (afterEquals && (character === '"' || character === "'")) quote = character;
    afterEquals = false;
  }
  return -1;
}

function parseAttributes(body) {
  const attributes = [];
  const pattern = /([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
  let match = pattern.exec(body);
  while (match) {
    const value = match[2] ?? match[3] ?? match[4] ?? "";
    attributes.push(makeAttribute(match[1], value));
    match = pattern.exec(body);
  }
  return attributes;
}

/** Parses markup into a document-shaped tree. Stands in for `DOMParser`. */
export function parseMarkup(markup) {
  const source = String(markup);
  const document = {
    nodeType: DOCUMENT_NODE,
    childNodes: [],
    documentElement: null,
    appendChild(child) {
      document.childNodes.push(child);
      return child;
    },
  };
  const stack = [document];
  const top = () => stack[stack.length - 1];
  const addText = (raw, nodeType = TEXT_NODE) => {
    if (raw === "") return;
    top().appendChild(makeText(nodeType === TEXT_NODE ? decodeEntities(raw) : raw, nodeType));
  };

  let cursor = 0;
  while (cursor < source.length) {
    const open = source.indexOf("<", cursor);
    if (open < 0) {
      addText(source.slice(cursor));
      break;
    }
    addText(source.slice(cursor, open));

    if (source.startsWith("<!--", open)) {
      const end = source.indexOf("-->", open);
      const stop = end < 0 ? source.length : end + 3;
      top().appendChild({
        nodeType: COMMENT_NODE,
        data: source.slice(open + 4, end < 0 ? source.length : end),
        childNodes: [],
      });
      cursor = stop;
      continue;
    }
    if (source.startsWith("<![CDATA[", open)) {
      const end = source.indexOf("]]>", open);
      const stop = end < 0 ? source.length : end;
      addText(source.slice(open + 9, stop), CDATA_NODE);
      cursor = end < 0 ? source.length : end + 3;
      continue;
    }
    if (source.startsWith("<!", open) || source.startsWith("<?", open)) {
      const end = source.indexOf(">", open);
      cursor = end < 0 ? source.length : end + 1;
      continue;
    }
    if (source.startsWith("</", open)) {
      const end = source.indexOf(">", open);
      const name = source.slice(open + 2, end < 0 ? source.length : end).trim().toLowerCase();
      for (let depth = stack.length - 1; depth >= 1; depth -= 1) {
        if (stack[depth].localName === name) {
          stack.length = depth;
          break;
        }
      }
      cursor = end < 0 ? source.length : end + 1;
      continue;
    }

    const end = findTagEnd(source, open);
    if (end < 0) {
      addText(source.slice(open));
      break;
    }
    let body = source.slice(open + 1, end);
    const selfClosing = body.endsWith("/");
    if (selfClosing) body = body.slice(0, -1);
    const space = body.search(/\s/);
    const tagName = (space < 0 ? body : body.slice(0, space)).toLowerCase();
    const element = makeElement(tagName, space < 0 ? [] : parseAttributes(body.slice(space)));
    top().appendChild(element);
    if (!document.documentElement && top() === document) document.documentElement = element;
    cursor = end + 1;

    if (RAW_TEXT_TAGS.has(tagName) && !selfClosing) {
      const closing = source.toLowerCase().indexOf(`</${tagName}`, cursor);
      const stop = closing < 0 ? source.length : closing;
      if (stop > cursor) element.appendChild(makeText(source.slice(cursor, stop)));
      const afterClose = closing < 0 ? source.length : source.indexOf(">", closing);
      cursor = afterClose < 0 ? source.length : afterClose + 1;
      continue;
    }
    if (!selfClosing && !VOID_TAGS.has(tagName)) stack.push(element);
  }
  return document;
}

/** A document that can make nodes. Stands in for the page's own `document`. */
export function createTargetDocument() {
  return {
    createElement(tagName) {
      return makeElement(String(tagName));
    },
    createTextNode(data) {
      return makeText(String(data));
    },
    createDocumentFragment() {
      const fragment = {
        nodeType: FRAGMENT_NODE,
        childNodes: [],
        appendChild(child) {
          fragment.childNodes.push(child);
          return child;
        },
      };
      return fragment;
    },
  };
}

function escapeText(value) {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Renders a node tree back to markup so a test can assert on what survived. */
export function serialize(node) {
  if (!node) return "";
  if (node.nodeType === TEXT_NODE || node.nodeType === CDATA_NODE) return escapeText(node.data);
  if (node.nodeType === COMMENT_NODE) return `<!--${node.data}-->`;
  const children = (node.childNodes || []).map((child) => serialize(child)).join("");
  if (node.nodeType !== ELEMENT_NODE) return children;
  const attributes = (node.attributes || [])
    .map((attribute) => ` ${attribute.name}="${escapeText(attribute.value)}"`)
    .join("");
  if (VOID_TAGS.has(node.localName)) return `<${node.localName}${attributes}/>`;
  return `<${node.localName}${attributes}>${children}</${node.localName}>`;
}

/** Collects the plain text a node tree carries. */
export function textOf(node) {
  if (!node) return "";
  if (node.nodeType === TEXT_NODE || node.nodeType === CDATA_NODE) return node.data;
  return (node.childNodes || []).map((child) => textOf(child)).join("");
}

/** Collects every element with the given tag name, the node itself included. */
export function findAll(node, tagName) {
  const wanted = tagName.toLowerCase();
  const found = [];
  const stack = [node];
  while (stack.length > 0) {
    const current = stack.pop();
    if (!current) continue;
    if (current.nodeType === ELEMENT_NODE && current.localName === wanted) found.push(current);
    const children = current.childNodes || [];
    for (let index = children.length - 1; index >= 0; index -= 1) stack.push(children[index]);
  }
  return found;
}
