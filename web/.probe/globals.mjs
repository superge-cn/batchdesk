// 在入口模块的静态导入之前跑：把 jsdom 的 window 挂到 globalThis，
// 让 React 19 / react-dom 认为自己在浏览器里。
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body><div id='root'></div></body></html>", {
  url: "http://localhost/",
  pretendToBeVisual: true
});
const { window } = dom;

globalThis.window = window;
globalThis.document = window.document;
// Node 22 的 globalThis.navigator 只有 getter，必须用 defineProperty 覆盖。
Object.defineProperty(globalThis, "navigator", {
  value: window.navigator,
  configurable: true,
  writable: true
});
globalThis.HTMLElement = window.HTMLElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;
globalThis.Event = window.Event;
globalThis.MouseEvent = window.MouseEvent;
globalThis.KeyboardEvent = window.KeyboardEvent;
globalThis.getComputedStyle = window.getComputedStyle.bind(window);
globalThis.requestAnimationFrame = (cb) => setTimeout(() => cb(Date.now()), 0);
globalThis.cancelAnimationFrame = (id) => clearTimeout(id);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__probeDom = dom;
