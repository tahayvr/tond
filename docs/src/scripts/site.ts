// The only JavaScript on the docs pages: theme toggle, the phone navigation
// drawer, copy buttons on code blocks, "on this page" highlighting and the
// Pagefind search dialog (loaded on first use, from this site).

const root = document.documentElement;

// ── Theme ──
function applyThemeLabel() {
	const dark = root.classList.contains('dark');
	document.querySelectorAll<HTMLButtonElement>('[data-theme-toggle]').forEach((b) => {
		b.setAttribute('aria-label', dark ? 'Switch to light theme' : 'Switch to dark theme');
	});
}
document.querySelectorAll('[data-theme-toggle]').forEach((button) =>
	button.addEventListener('click', () => {
		const dark = !root.classList.contains('dark');
		root.classList.toggle('dark', dark);
		try {
			localStorage.setItem('tond-docs-theme', dark ? 'dark' : 'light');
		} catch {
			/* private mode: the choice lasts for this page */
		}
		applyThemeLabel();
	})
);
applyThemeLabel();

// ── Navigation drawer (phones) ──
const sidebar = document.getElementById('sidebar');
const scrim = document.querySelector<HTMLElement>('[data-menu-close]');
const menuButton = document.querySelector<HTMLButtonElement>('[data-menu-toggle]');
function setMenu(open: boolean) {
	sidebar?.classList.toggle('open', open);
	if (scrim) scrim.hidden = !open;
	menuButton?.setAttribute('aria-expanded', String(open));
	menuButton?.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
	document.body.classList.toggle('menu-open', open);
}
menuButton?.addEventListener('click', () => setMenu(!sidebar?.classList.contains('open')));
scrim?.addEventListener('click', () => setMenu(false));
sidebar?.querySelector('[aria-current="page"]')?.scrollIntoView({ block: 'center' });

// ── Copy buttons ──
document.querySelectorAll<HTMLPreElement>('.prose pre').forEach((pre) => {
	const wrapper = document.createElement('div');
	wrapper.className = 'code-block';
	pre.replaceWith(wrapper);
	wrapper.append(pre);
	const button = document.createElement('button');
	button.type = 'button';
	button.className = 'copy-button';
	button.textContent = 'Copy';
	button.setAttribute('aria-label', 'Copy code to clipboard');
	button.addEventListener('click', async () => {
		const code = pre.querySelector('code')?.innerText ?? pre.innerText;
		try {
			await navigator.clipboard.writeText(code.replace(/\n$/, ''));
			button.textContent = 'Copied';
		} catch {
			button.textContent = 'Press Ctrl+C';
			const range = document.createRange();
			range.selectNodeContents(pre);
			getSelection()?.removeAllRanges();
			getSelection()?.addRange(range);
		}
		setTimeout(() => (button.textContent = 'Copy'), 2000);
	});
	wrapper.append(button);
});

// ── "On this page" highlighting ──
const tocLinks = new Map<string, HTMLAnchorElement>();
document
	.querySelectorAll<HTMLAnchorElement>('[data-toc-link]')
	.forEach((a) => tocLinks.set(a.dataset.tocLink ?? '', a));
if (tocLinks.size > 0 && 'IntersectionObserver' in window) {
	const visible = new Set<string>();
	const headings = [...tocLinks.keys()]
		.map((id) => document.getElementById(id))
		.filter((h): h is HTMLElement => h !== null);
	const observer = new IntersectionObserver(
		(entries) => {
			for (const e of entries) {
				if (e.isIntersecting) visible.add(e.target.id);
				else visible.delete(e.target.id);
			}
			const current = headings.find((h) => visible.has(h.id));
			if (!current) return;
			tocLinks.forEach((a, id) => a.classList.toggle('active', id === current.id));
		},
		{ rootMargin: '0px 0px -70% 0px' }
	);
	headings.forEach((h) => observer.observe(h));
}

// ── Search (Pagefind, fully static) ──
const dialog = document.querySelector<HTMLDialogElement>('[data-search-dialog]');
const searchRoot = document.getElementById('search-ui');
let searchLoaded: Promise<void> | null = null;

function loadSearch(): Promise<void> {
	if (searchLoaded) return searchLoaded;
	const base = searchRoot?.dataset.base ?? '';
	searchLoaded = new Promise<void>((resolve) => {
		const css = document.createElement('link');
		css.rel = 'stylesheet';
		css.href = `${base}/pagefind/pagefind-ui.css`;
		document.head.append(css);
		const script = document.createElement('script');
		script.src = `${base}/pagefind/pagefind-ui.js`;
		script.onload = () => {
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			const PagefindUI = (window as any).PagefindUI;
			new PagefindUI({
				element: '#search-ui',
				showSubResults: true,
				showImages: false,
				resetStyles: false,
				autofocus: true
			});
			resolve();
		};
		script.onerror = () => {
			document.querySelector<HTMLElement>('[data-search-fallback]')?.removeAttribute('hidden');
			resolve();
		};
		document.head.append(script);
	});
	return searchLoaded;
}

function openSearch() {
	if (!dialog) return;
	dialog.showModal();
	loadSearch().then(() => dialog.querySelector<HTMLInputElement>('input')?.focus());
}
document.querySelectorAll('[data-search-open]').forEach((b) => b.addEventListener('click', openSearch));
document.querySelector('[data-search-close]')?.addEventListener('click', () => dialog?.close());
dialog?.addEventListener('click', (e) => {
	if (e.target === dialog) dialog.close();
});
document.addEventListener('keydown', (e) => {
	const target = e.target as HTMLElement | null;
	const typing = target?.closest('input, textarea, [contenteditable="true"]');
	if (!typing && (e.key === '/' || ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k'))) {
		e.preventDefault();
		openSearch();
	}
	if (e.key === 'Escape' && sidebar?.classList.contains('open')) setMenu(false);
});
