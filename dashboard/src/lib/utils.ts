import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

/**
 * Return `url` normalised if it is an absolute `https:` URL (or `http:` when
 * `allowHttp` is set), otherwise `undefined`.
 *
 * Use it for every `href` built from API data (PR/commit links, app and
 * preview URLs, integration links): a stored `javascript:` or `data:` URL then
 * renders as plain text instead of a clickable script.
 */
export function safeUrl(
	url: string | null | undefined,
	{ allowHttp = false }: { allowHttp?: boolean } = {}
): string | undefined {
	if (!url) return undefined;
	let parsed: URL;
	try {
		parsed = new URL(url.trim());
	} catch {
		return undefined;
	}
	if (parsed.protocol === 'https:' || (allowHttp && parsed.protocol === 'http:')) {
		return parsed.href;
	}
	return undefined;
}

export type WithoutChild<T> = T extends { child?: any } ? Omit<T, 'child'> : T;
export type WithoutChildren<T> = T extends { children?: any } ? Omit<T, 'children'> : T;
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & { ref?: U | null };

/** Hide the password in a `scheme://user:password@host...` connection string. */
export function maskUrlPassword(url: string): string {
	return url.replace(/^([a-z][a-z0-9+.-]*:\/\/[^:/@\s]*:)([^@\s]+)(@)/i, '$1••••••••$3');
}
