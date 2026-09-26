import { describe, expect, it } from 'vitest';
import { commitUrl, repoWebUrl } from './commit-links';

const SHA = 'abcdef1234567890';

describe('commit links', () => {
	it('links GitHub, GitLab, Gitea and Bitbucket commits', () => {
		expect(commitUrl('https://github.com/o/r.git', SHA)).toBe(
			`https://github.com/o/r/commit/${SHA}`
		);
		expect(commitUrl('git@gitlab.com:group/sub/r.git', SHA)).toBe(
			`https://gitlab.com/group/sub/r/-/commit/${SHA}`
		);
		expect(commitUrl('https://git.example.com/o/r', SHA)).toBe(
			`https://git.example.com/o/r/commit/${SHA}`
		);
		expect(commitUrl('https://bitbucket.org/o/r', SHA)).toBe(
			`https://bitbucket.org/o/r/commits/${SHA}`
		);
		expect(commitUrl('ssh://git@github.com:22/o/r.git', SHA)).toBe(
			`https://github.com/o/r/commit/${SHA}`
		);
	});

	it('drops credentials', () => {
		expect(repoWebUrl('https://oauth2:glpat-secret@gitlab.com/g/r.git')).toBe(
			'https://gitlab.com/g/r'
		);
	});

	it('refuses anything else', () => {
		expect(commitUrl(null, SHA)).toBeNull();
		expect(commitUrl('https://github.com/o/r', null)).toBeNull();
		expect(commitUrl('https://github.com/o/r', 'main')).toBeNull();
		expect(commitUrl('javascript:alert(1)//x/y', SHA)).toBeNull();
		expect(commitUrl('https://github.com/only-owner', SHA)).toBeNull();
		expect(commitUrl('nginx:latest', SHA)).toBeNull();
	});
});
