/**
 * The API envelope as tond-server sends it (see ApiResponse in $lib/api):
 *   { success: true, data }
 *   { success: false, data: null, error: { code, message, hint?, field? } }
 * Codes follow the server's generic mapping for a status (error.rs
 * `code_for_status`) unless a handler passes its own.
 */

export interface MockErrorExtra {
	hint?: string;
	field?: string;
}

/** The server's generic code for an HTTP status. */
export function codeForStatus(status: number, message = ''): string {
	switch (status) {
		case 400:
		case 422:
			return 'INVALID_INPUT';
		case 401:
			// Auth endpoints answer INVALID_CREDENTIALS for a wrong password or
			// code; anything else is an expired session.
			return message === 'Unauthorized' ? 'SESSION_EXPIRED' : 'INVALID_CREDENTIALS';
		case 403:
			return 'FORBIDDEN';
		case 404:
			return 'NOT_FOUND';
		case 405:
			return 'METHOD_NOT_ALLOWED';
		case 409:
			return 'CONFLICT';
		case 413:
			return 'PAYLOAD_TOO_LARGE';
		case 429:
			return 'RATE_LIMITED';
		case 503:
			return 'SERVICE_UNAVAILABLE';
		default:
			return status >= 400 && status < 500 ? 'INVALID_INPUT' : 'INTERNAL';
	}
}

export function okBody<T>(data: T) {
	return { success: true, data };
}

export function errorBody(
	message: string,
	status = 400,
	code?: string,
	extra: MockErrorExtra = {}
) {
	return {
		success: false,
		data: null,
		error: { code: code ?? codeForStatus(status, message), message, ...extra }
	};
}
