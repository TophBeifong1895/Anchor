import { test, type Page } from '@playwright/test';

/** Register before fixture routes: a missing mock must never reach the development Host. */
export async function blockUnmockedApi(page: Page): Promise<void> {
  const origin = new URL(test.info().project.use.baseURL!).origin;
  await page.route('**/*', route => {
    const request = route.request(), url = new URL(request.url()), path = url.pathname;
    if (url.origin === origin && request.method() === 'GET'
      && (path === '/' || /^\/(src|@vite|@id|@fs|node_modules|assets)\//.test(path) || path === '/@react-refresh' || path === '/favicon.ico')) return route.continue();
    return route.fulfill({ status: 404, json: { error: `Unmocked API: ${request.method()} ${path}` } });
  });
}
