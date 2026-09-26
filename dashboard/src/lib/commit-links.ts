// Link a commit to its page on the Git host, from the app's repository URL.
//
//   https://github.com/o/r(.git)        → https://github.com/o/r/commit/<sha>
//   git@gitlab.com:g/sub/r.git          → https://gitlab.com/g/sub/r/-/commit/<sha>
//   https://git.example.com/o/r (Gitea) → https://git.example.com/o/r/commit/<sha>
//   https://bitbucket.org/o/r           → https://bitbucket.org/o/r/commits/<sha>
//
// Credentials in the URL are dropped; anything that isn't a plausible web
// repository gives null. The result goes through safeUrl (https only).

import { safeUrl } from '$lib/utils';

/** The repository's web address, or null. */
export function repoWebUrl(repoUrl: string | null | undefined): string | null {
	if (!repoUrl) return null;
	let url = repoUrl.trim();
	// scp-like SSH: git@host:path
	const scp = /^[\w.-]+@([\w.-]+):(?!\/\/)(.+)$/.exec(url);
	if (scp) url = `https://${scp[1]}/${scp[2]}`;
	url = url.replace(/^ssh:\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\//, 'https://$1/');
	url = url.replace(/^git:\/\//, 'https://').replace(/^http:\/\//, 'https://');
	let parsed: URL;
	try {
		parsed = new URL(url);
	} catch {
		return null;
	}
	if (parsed.protocol !== 'https:') return null;
	const path = parsed.pathname.replace(/\/+$/, '').replace(/\.git$/, '');
	if (path.split('/').filter(Boolean).length < 2) return null;
	return safeUrl(`https://${parsed.host}${path}`) ?? null;
}

/** Web page of `sha` in the repository, or null. */
export function commitUrl(
	repoUrl: string | null | undefined,
	sha: string | null | undefined
): string | null {
	if (!sha || !/^[0-9a-f]{7,64}$/i.test(sha)) return null;
	const base = repoWebUrl(repoUrl);
	if (!base) return null;
	const host = new URL(base).host.toLowerCase();
	const suffix = host.includes('gitlab')
		? `/-/commit/${sha}`
		: host.includes('bitbucket')
			? `/commits/${sha}`
			: `/commit/${sha}`;
	return safeUrl(base + suffix) ?? null;
}
