import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import UploadCertificateDialog from './upload-certificate-dialog.svelte';

const { uploadCertificate } = vi.hoisted(() => ({ uploadCertificate: vi.fn() }));

vi.mock('$lib/api', () => ({ uploadCertificate }));

const CERT = '-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----';
const KEY = '-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----';

async function fill(user: ReturnType<typeof userEvent.setup>) {
	await user.click(screen.getByLabelText(/certificate chain/i));
	await user.paste(CERT);
	await user.click(screen.getByLabelText(/private key/i));
	await user.paste(KEY);
	await user.click(screen.getByRole('button', { name: /upload certificate/i }));
}

describe('UploadCertificateDialog', () => {
	beforeEach(() => uploadCertificate.mockReset());

	it('uploads and reports the domain', async () => {
		uploadCertificate.mockResolvedValue(null);
		const onUploaded = vi.fn();
		const user = userEvent.setup();
		render(UploadCertificateDialog, { props: { open: true, onUploaded } });
		await user.type(screen.getByLabelText(/^domain$/i), '*.Corp.example.com');
		await fill(user);
		expect(onUploaded).toHaveBeenCalledWith(null, '*.corp.example.com');
	});
});
