import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode
} from "react";
import zhCN from "./locales/zh-CN.json";
import enUS from "./locales/en-US.json";
import { getConfig, setConfig } from "./lib/ipc";

/** 设计 09 §1：只承诺 zh-CN（默认）与 en-US，不做 RTL 与地区变体。 */
export type Lang = "zh-CN" | "en-US";
/** 设计 03 §4.1 的 theme 枚举；`system` 在运行时解析成明/暗。 */
export type ThemePref = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

export type MessageKey = keyof typeof zhCN;

type Bundle = Record<string, string>;

const BUNDLES: Record<Lang, Bundle> = {
  "zh-CN": zhCN as Bundle,
  "en-US": enUS as Bundle
};

export const DEFAULT_LANG: Lang = "zh-CN";
export const LANGS: Lang[] = ["zh-CN", "en-US"];

/** Vite 注入的 DEV 标记；本仓库没有 vite/client 类型声明，这里按需读取。 */
const DEV = (import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV === true;

/**
 * 占位符插值（设计 09 §2.4）：整句模板 + `{name}` 参数，不做片段拼接。
 * 参数缺失时保留占位符原样——比渲染出 `undefined` 更容易在验收时发现。
 */
export function interpolate(
  template: string,
  params?: Record<string, string | number>
): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = params[name];
    return value === undefined ? match : String(value);
  });
}

/**
 * 取文案：当前语言缺 key 时回退 zh-CN（设计 09 §4「语言包加载失败回退」），
 * 两边都缺则返回 key 本身——门禁 `scripts/check_locales.py` 保证这种情况不会进构建。
 */
export function translate(
  lang: Lang,
  key: string,
  params?: Record<string, string | number>
): string {
  const primary = BUNDLES[lang]?.[key];
  if (primary !== undefined) return interpolate(primary, params);
  const fallback = BUNDLES[DEFAULT_LANG][key];
  if (fallback !== undefined) {
    if (DEV) {
      console.warn(`[i18n] missing key "${key}" in ${lang}; fell back to ${DEFAULT_LANG}`);
    }
    return interpolate(fallback, params);
  }
  console.error(`[i18n] unknown message key: ${key}`);
  return key;
}

export function isLang(value: unknown): value is Lang {
  return value === "zh-CN" || value === "en-US";
}

export function isThemePref(value: unknown): value is ThemePref {
  return value === "system" || value === "light" || value === "dark";
}

function systemTheme(): ResolvedTheme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function resolveTheme(pref: ThemePref): ResolvedTheme {
  return pref === "system" ? systemTheme() : pref;
}

interface I18nContextValue {
  lang: Lang;
  theme: ThemePref;
  resolvedTheme: ResolvedTheme;
  setLang: (next: Lang) => void;
  setTheme: (next: ThemePref) => void;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

export interface I18nProviderProps {
  children: ReactNode;
  /** 测试与首帧用；不传则每个 Provider 挂载时从 `getConfig("general")` 读回上次选择。 */
  initialLang?: Lang;
  initialTheme?: ThemePref;
  /** 测试替身：注入持久化实现，默认走 setConfig。 */
  persist?: (scope: "general", patch: Record<string, unknown>) => Promise<unknown>;
}

/**
 * 语言与主题的唯一来源。
 *
 * 设计 09 §4：默认 zh-CN、切换即时生效不重启、选择写 `general.language`；
 * 设计 03 §4.1：主题同属通用组。文案与主题都只在这里落地，
 * 组件不得自己读 localStorage / 自己拼字符串。
 */
export function I18nProvider({
  children,
  initialLang,
  initialTheme,
  persist
}: I18nProviderProps) {
  const [lang, setLangState] = useState<Lang>(initialLang ?? DEFAULT_LANG);
  const [theme, setThemeState] = useState<ThemePref>(initialTheme ?? "light");
  const [resolvedTheme, setResolvedTheme] = useState<ResolvedTheme>(resolveTheme(theme));

  // 首帧回读：只读一次，失败按默认档（设计 09 §4 回退，不显示空白界面）。
  useEffect(() => {
    if (initialLang && initialTheme) return;
    let disposed = false;
    getConfig("general")
      .then((view) => {
        if (disposed) return;
        const values = (view as { values?: Record<string, { value?: unknown }> }).values ?? {};
        const storedLang = values.language?.value;
        const storedTheme = values.theme?.value;
        if (!initialLang && isLang(storedLang)) setLangState(storedLang);
        if (!initialTheme && isThemePref(storedTheme)) setThemeState(storedTheme);
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
    };
  }, [initialLang, initialTheme]);

  const write = useCallback(
    (patch: Record<string, unknown>) => {
      const writer = persist ?? setConfig;
      writer("general", patch).catch((error: unknown) => {
        // 只读档（E1007）或存储不可写时本地仍然生效：不因为写盘失败把界面卡住，
        // 也不假装保存成功——调用方在设置页可见 warning。
        console.warn("[i18n] failed to persist general settings", error);
      });
    },
    [persist]
  );

  const setLang = useCallback(
    (next: Lang) => {
      setLangState(next);
      write({ language: next });
    },
    [write]
  );

  const setTheme = useCallback(
    (next: ThemePref) => {
      setThemeState(next);
      write({ theme: next });
    },
    [write]
  );

  // `lang` 属性随语言切换（设计 09 §3 可访问性）；主题落 dataset 供 CSS 令牌切换。
  useEffect(() => {
    if (typeof document !== "undefined") document.documentElement.lang = lang;
  }, [lang]);

  useEffect(() => {
    setResolvedTheme(resolveTheme(theme));
    if (theme !== "system") return;
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setResolvedTheme(systemTheme());
    query.addEventListener?.("change", onChange);
    return () => query.removeEventListener?.("change", onChange);
  }, [theme]);

  useEffect(() => {
    if (typeof document !== "undefined") document.documentElement.dataset.theme = resolvedTheme;
  }, [resolvedTheme]);

  const t = useCallback(
    (key: MessageKey, params?: Record<string, string | number>) => translate(lang, key, params),
    [lang]
  );

  const value = useMemo<I18nContextValue>(
    () => ({ lang, theme, resolvedTheme, setLang, setTheme, t }),
    [lang, theme, resolvedTheme, setLang, setTheme, t]
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** 取当前语言/主题与取词函数；未包 Provider 时按默认中文取词（便于单元测试单挂组件）。 */
export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext);
  if (context) return context;
  return {
    lang: DEFAULT_LANG,
    theme: "light",
    resolvedTheme: "light",
    setLang: () => undefined,
    setTheme: () => undefined,
    t: (key: MessageKey, params?: Record<string, string | number>) =>
      translate(DEFAULT_LANG, key, params)
  };
}
