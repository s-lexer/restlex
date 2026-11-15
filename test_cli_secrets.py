"""Проверка вычистки секретов в export-config --no-secrets.
Запуск: python test_cli_secrets.py

Импортируем боевые константы и функции из restlex.py, а не копию: копия
проходила зелёной мимо реальных утечек (webserver tg_token, api_token),
потому что проверяла сама себя. Импорт безопасен — GUI поднимается только
под `if __name__ == "__main__"`.
"""
import json

from restlex import (
    _EXPORT_SECRET_GLOBAL_KEYS,
    _EXPORT_SECRET_PROFILE_KEYS,
    _EXPORT_SECRET_WEB_KEYS,
    _strip_export_secrets,
    _strip_url_password,
    WebServerManager,
)


def test_url_password():
    # userinfo вырезается целиком
    assert _strip_url_password(
        "rest:http://lex:Se%40cret@192.168.1.10:8001/repo"
    ) == "rest:http://192.168.1.10:8001/repo"
    # сырой '@' внутри пароля — хвост не должен уцелеть
    out = _strip_url_password("rest:http://u:p@ss@host:8001/r")
    assert out == "rest:http://host:8001/r", out
    assert "ss" not in out.split("host")[0]
    # IPv6-хост
    assert _strip_url_password("rest:http://u:p@[::1]:8000/r") == "rest:http://[::1]:8000/r"
    # без userinfo — не трогаем
    assert _strip_url_password("rest:http://host:8001/repo") == "rest:http://host:8001/repo"
    # '@' только в пути
    assert _strip_url_password("rest:http://host/repo@2") == "rest:http://host/repo@2"
    # схема без '//' (sftp) — не трогаем
    assert _strip_url_password("sftp:user@host:/srv/repo") == "sftp:user@host:/srv/repo"
    assert _strip_url_password("C:\\backup\\repo") == "C:\\backup\\repo"
    assert _strip_url_password("") == ""
    assert _strip_url_password(None) is None


def test_known_secret_fields_are_listed():
    """Секреты, которые реально есть в конфиге, должны быть в списках стрипа."""
    for k in ("repo_password", "rest_pass", "sftp_p", "s3_s", "b2_k", "az_k"):
        assert k in _EXPORT_SECRET_PROFILE_KEYS, k
    for k in ("tg_token", "report_api_key", "update_api_key", "api_token"):
        assert k in _EXPORT_SECRET_GLOBAL_KEYS, k
    # ловим появление новых секретных полей в дефолтах веб-сервера
    for k in WebServerManager._DEFAULT:
        if any(t in k for t in ("password", "token", "key", "secret")):
            assert k in _EXPORT_SECRET_WEB_KEYS, f"секрет веб-сервера не стрипается: {k}"


def test_strip_secrets():
    data = {
        "global": {
            "tg_token": "GTG!", "report_api_key": "RAK!", "update_api_key": "UAK!",
            "api_token": "ATK!", "report_url": "http://rep", "tg_fatal": True,
        },
        "profiles": {
            "p1": {
                "name": "srv", "repo_password": "RPW!", "s3_s": "S3S!", "sftp_p": "SFP!",
                "b2_k": "B2K!", "az_k": "AZK!",
                "repo_type": "s3", "sources": ["C:\\data"],
            },
            "p2": {
                "name": "rest", "repo_type": "rest",
                "rest_user": "lex", "rest_pass": "RP2!",
                "repo_path": "rest:http://lex:RP2!@host:8001/r",
            },
            "broken": "не словарь — не должен ронять",
        },
        "webserver": {
            "admin_password": "WAP!", "viewer_password": "WVP!",
            "api_key": "WAK!", "secret_key": "WSK!", "tg_token": "WTG!",
            "port": 8798,
        },
    }
    out = _strip_export_secrets(data)

    # несекретное — на месте
    assert out["global"]["report_url"] == "http://rep"
    assert out["global"]["tg_fatal"] is True
    assert out["profiles"]["p1"]["sources"] == ["C:\\data"]
    assert out["profiles"]["p1"]["name"] == "srv"
    assert out["webserver"]["port"] == 8798
    assert out["profiles"]["broken"] == "не словарь — не должен ронять"

    # исходник не мутирован
    assert data["global"]["tg_token"] == "GTG!"
    assert data["profiles"]["p1"]["repo_password"] == "RPW!"

    # ни один секрет не утёк в сериализованный вид
    blob = json.dumps(out, ensure_ascii=False)
    for leak in ("GTG!", "RAK!", "UAK!", "ATK!", "RPW!", "S3S!", "SFP!",
                 "B2K!", "AZK!", "RP2!", "WAP!", "WVP!", "WAK!", "WSK!", "WTG!"):
        assert leak not in blob, f"утёк секрет: {leak}"


def test_empty():
    assert _strip_export_secrets({}) == {"global": {}, "profiles": {}, "webserver": {}}


if __name__ == "__main__":
    test_url_password()
    test_known_secret_fields_are_listed()
    test_strip_secrets()
    test_empty()
    print("OK")
