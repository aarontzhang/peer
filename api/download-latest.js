export default function handler(_req, res) {
  const url = process.env.PEER_MACOS_DOWNLOAD_URL;
  if (!url) {
    res.statusCode = 404;
    res.setHeader('content-type', 'text/plain; charset=utf-8');
    res.end('Latest macOS download is not configured yet.');
    return;
  }
  res.statusCode = 302;
  res.setHeader('location', url);
  res.end();
}
