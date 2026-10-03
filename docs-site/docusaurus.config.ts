import { themes as prismThemes } from "prism-react-renderer";
import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";

// Caravel's documentation (M0.7, H-12, DEC-100). The docs are the site
// (routeBasePath "/"); the landing page lives at caravel-tau.vercel.app.
const site = "https://caravel-docs.vercel.app";
const repo = "https://github.com/wmendes/caravel";

const config: Config = {
  title: "Caravel docs",
  tagline: "Appchains with their own rules, settled on Stellar.",
  favicon: "img/favicon.ico",
  url: site,
  baseUrl: "/",
  organizationName: "wmendes",
  projectName: "caravel",
  trailingSlash: false,
  onBrokenLinks: "throw",
  onBrokenAnchors: "throw",
  markdown: {
    // .md pages are CommonMark, so a lane file's ${...} and <name> are text; .mdx pages are MDX.
    format: "detect",
    hooks: { onBrokenMarkdownLinks: "throw" },
  },
  i18n: { defaultLocale: "en", locales: ["en"] },
  headTags: [
    { tagName: "link", attributes: { rel: "icon", href: "/img/icon.svg", type: "image/svg+xml" } },
    { tagName: "link", attributes: { rel: "apple-touch-icon", href: "/img/apple-touch-icon.png" } },
    { tagName: "link", attributes: { rel: "preload", href: "/fonts/schibsted-grotesk-latin.woff2", as: "font", type: "font/woff2", crossorigin: "anonymous" } },
  ],
  presets: [
    [
      "classic",
      {
        docs: {
          routeBasePath: "/",
          sidebarPath: "./sidebars.ts",
          editUrl: `${repo}/edit/main/docs-site/`,
          showLastUpdateTime: false,
          breadcrumbs: true,
        },
        blog: false,
        theme: { customCss: "./src/css/custom.css" },
        sitemap: { changefreq: "weekly", priority: 0.5 },
      } satisfies Preset.Options,
    ],
  ],
  themes: [
    [
      "@easyops-cn/docusaurus-search-local",
      {
        hashed: true,
        docsRouteBasePath: "/",
        indexBlog: false,
        indexPages: false,
        highlightSearchTermsOnTargetPage: true,
        searchResultLimits: 8,
        explicitSearchResultPath: true,
      },
    ],
  ],
  themeConfig: {
    image: "img/og-docs.png",
    metadata: [
      { name: "description", content: "Caravel's documentation: declare an appchain with its own rules in one lane file, plan it, apply it, and settle it on Stellar. Testnet only, not audited." },
      { name: "twitter:card", content: "summary_large_image" },
      { name: "theme-color", content: "#081E22" },
    ],
    colorMode: { defaultMode: "dark", respectPrefersColorScheme: true },
    announcementBar: {
      id: "testnet",
      content: "Caravel is testnet software and has not been audited.",
      isCloseable: false,
    },
    navbar: {
      title: "",
      logo: { alt: "Caravel", src: "img/logo.svg", srcDark: "img/logo-dark.svg", href: "/", width: 128, height: 29 },
      items: [
        { type: "docSidebar", sidebarId: "docs", position: "left", label: "Docs" },
        { to: "/reference/lane-file", position: "left", label: "Lane file" },
        { to: "/reference/cli", position: "left", label: "CLI" },
        { href: "https://caravel-tau.vercel.app", position: "right", label: "Caravel" },
        { href: repo, position: "right", label: "GitHub" },
      ],
    },
    footer: {
      style: "light",
      links: [
        {
          title: "Docs",
          items: [
            { label: "Get started", to: "/" },
            { label: "Lane file", to: "/reference/lane-file" },
            { label: "CLI", to: "/reference/cli" },
          ],
        },
        {
          title: "Project",
          items: [
            { label: "GitHub", href: repo },
            { label: "Specification", href: `${repo}/blob/main/docs/CARAVEL_SPEC.md` },
            { label: "Measured results", href: `${repo}/blob/main/docs/RESULTS.md` },
            { label: "Security review", href: `${repo}/blob/main/docs/SECURITY.md` },
          ],
        },
        {
          title: "Caravel",
          items: [
            { label: "Home", href: "https://caravel-tau.vercel.app" },
            { label: "Caravel Perps on testnet", href: "https://35-224-76-64.sslip.io" },
          ],
        },
      ],
      copyright: "Open source under MIT or Apache-2.0. Testnet only, not audited.",
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.vsDark,
      additionalLanguages: ["toml", "bash", "rust", "json", "diff"],
    },
    tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 3 },
  } satisfies Preset.ThemeConfig,
};

export default config;
