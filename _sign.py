"""EV code-signing через sign-service. Адрес и токен — НЕ в исходниках:
переменные окружения RESTLEX_SIGN_URL / RESTLEX_SIGN_TOKEN или локальный
%USERPROFILE%\\.restlex\\build-secrets.json (sign_url, sign_token)."""
import sys, urllib.request, urllib.error, uuid, os, json, base64, hashlib


def _secret(env, key):
    v = os.environ.get(env, "")
    if v:
        return v
    try:
        p = os.path.join(os.path.expanduser("~"), ".restlex", "build-secrets.json")
        with open(p, encoding="utf-8-sig") as f:
            return json.load(f).get(key, "")
    except (OSError, ValueError):
        return ""


BASE_URL  = _secret("RESTLEX_SIGN_URL", "sign_url")
SIGN_URL  = f"{BASE_URL}/sign"
DELTA_URL = f"{BASE_URL}/sign-delta"
TOKEN     = _secret("RESTLEX_SIGN_TOKEN", "sign_token")
if not BASE_URL or not TOKEN:
    sys.exit("_sign.py: не заданы RESTLEX_SIGN_URL/RESTLEX_SIGN_TOKEN (или ~/.restlex/build-secrets.json)")

# Канал до sign-service идёт через WG-туннель (~140 КБ/с, RTT ~160 мс): сам сервис быстрый
# (локально на RDS-WEB 50 МБ = 9 c), упираемся только в передачу файла.
TIMEOUT = 1800


def _post(url, path, data):
    boundary = uuid.uuid4().hex.encode()
    fname = os.path.basename(path).encode()
    body = (
        b"--" + boundary + b"\r\n"
        b'Content-Disposition: form-data; name="file"; filename="' + fname + b'"\r\n'
        b"Content-Type: application/octet-stream\r\n\r\n"
        + data +
        b"\r\n--" + boundary + b"--\r\n"
    )
    req = urllib.request.Request(
        url, data=body,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Content-Type": f"multipart/form-data; boundary={boundary.decode()}",
        },
        method="POST"
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return resp.read()


def _apply_patches(original: bytes, size: int, patches: list) -> bytes:
    out = bytearray(original)
    if size > len(out):
        out.extend(b"\0" * (size - len(out)))
    else:
        del out[size:]
    for p in patches:
        blob = base64.b64decode(p["b64"])
        off = int(p["off"])
        out[off:off + len(blob)] = blob
    if len(out) != size:
        raise RuntimeError(f"patch result {len(out)} != expected {size} bytes")
    return bytes(out)


def sign(path):
    with open(path, "rb") as f:
        data = f.read()

    # /sign-delta возвращает только изменившиеся участки (~10 КБ вместо всего файла) —
    # на медленном канале это половина времени. Старый сервер его не знает → 404,
    # тогда работаем как раньше через /sign.
    try:
        meta = json.loads(_post(DELTA_URL, path, data).decode())
        signed = _apply_patches(data, int(meta["size"]), meta["patches"])
        # совпадения длины мало: сверяем с хешем того, что реально подписал jsign
        want = meta.get("sha256")
        if want and hashlib.sha256(signed).hexdigest() != want:
            raise RuntimeError("delta-сборка не совпала с sha256 сервера — файл не записан")
    except urllib.error.HTTPError as e:
        if e.code != 404:
            raise
        signed = _post(SIGN_URL, path, data)

    # Подписанный файл не может быть меньше исходного (сертификат только
    # добавляет байты) — абсолютная константа 100_000 ошибочно резала мелкие
    # exe вроде RestLex-Simple.exe (~60-70 КБ).
    if len(signed) < len(data):
        raise RuntimeError(f"Response smaller than input ({len(signed)} < {len(data)} bytes) — signing failed")
    with open(path, "wb") as f:
        f.write(signed)
    print(f"Signed: {path} ({len(signed):,} bytes)")


if __name__ == "__main__":
    for p in sys.argv[1:]:
        sign(p)
