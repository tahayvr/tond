// The only JavaScript on the landing page: the theme toggle (same saved
// choice as the docs) and the install command's copy button.

const root = document.documentElement;

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

document.querySelectorAll<HTMLElement>('[data-copy-root]').forEach((box) => {
	const button = box.querySelector<HTMLButtonElement>('[data-copy]');
	const text = box.querySelector<HTMLElement>('[data-copy-text]');
	const label = box.querySelector<HTMLElement>('[data-copy-label]');
	const status = box.querySelector<HTMLElement>('[data-copy-status]');
	if (!button || !text || !label) return;
	let timer: ReturnType<typeof setTimeout> | undefined;
	const say = (message: string) => {
		label.textContent = message;
		if (status) status.textContent = message === 'Copy' ? '' : message;
	};
	button.addEventListener('click', async () => {
		try {
			await navigator.clipboard.writeText(text.textContent?.trim() ?? '');
			say('Copied');
			button.classList.add('copied');
		} catch {
			// No clipboard access (http, old browser): select it for Ctrl+C.
			say('Press Ctrl+C');
			const range = document.createRange();
			range.selectNodeContents(text);
			getSelection()?.removeAllRanges();
			getSelection()?.addRange(range);
		}
		clearTimeout(timer);
		timer = setTimeout(() => {
			say('Copy');
			button.classList.remove('copied');
		}, 2000);
	});
});
