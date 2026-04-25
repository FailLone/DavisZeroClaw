from unittest.mock import patch, MagicMock

from crawl4ai_adapter.engines import (
    ExtractResult,
    extract_openrouter_llm,
    extract_trafilatura,
)


def test_extract_result_has_required_fields():
    r = ExtractResult(markdown="hello", metadata={}, engine="test", warnings=[])
    assert r.markdown == "hello"
    assert r.metadata == {}
    assert r.engine == "test"
    assert r.warnings == []
    assert r.is_empty() is False


def test_extract_result_empty_detection():
    r = ExtractResult(markdown="", metadata={}, engine="test", warnings=["no content"])
    assert r.is_empty() is True
    r2 = ExtractResult(markdown="   \n\t  ", metadata={}, engine="test", warnings=[])
    assert r2.is_empty() is True


SAMPLE_HTML = """<!DOCTYPE html>
<html><head><title>Hello</title></head>
<body>
  <article>
    <h1>My Post</h1>
    <p>This is the first paragraph of the article body.</p>
    <p>This is the second paragraph with more detail about the topic.</p>
    <pre><code>print("hello")</code></pre>
  </article>
  <aside>Sidebar ad — should be dropped</aside>
</body></html>"""


def test_trafilatura_returns_markdown():
    r = extract_trafilatura(SAMPLE_HTML)
    assert r.engine == "trafilatura"
    assert not r.is_empty()
    assert "My Post" in r.markdown
    assert "first paragraph" in r.markdown
    assert "Sidebar ad" not in r.markdown


def test_trafilatura_empty_html_returns_warning():
    r = extract_trafilatura("<html><body></body></html>")
    assert r.is_empty()
    assert r.warnings, "empty extraction should carry a warning"


def test_openrouter_llm_returns_markdown_from_choices():
    fake_response = MagicMock()
    fake_response.status_code = 200
    fake_response.json.return_value = {
        "choices": [{"message": {"content": "# Title\n\nBody text."}}]
    }
    fake_response.raise_for_status = lambda: None

    with patch("httpx.Client") as client_cls:
        client_cls.return_value.__enter__.return_value.post.return_value = fake_response
        r = extract_openrouter_llm(
            "<html><body><p>hi</p></body></html>",
            {
                "base_url": "https://openrouter.ai/api/v1",
                "api_key": "sk-test",
                "model": "google/gemini-2.0-flash-001",
                "timeout_secs": 30,
            },
        )
    assert r.engine == "openrouter-llm"
    assert r.markdown.startswith("# Title")
    assert r.warnings == []


def test_openrouter_llm_surfaces_http_failure_as_warning():
    fake_response = MagicMock()
    fake_response.status_code = 500
    fake_response.text = "upstream error"
    def raise_for_status():
        import httpx
        raise httpx.HTTPStatusError("500", request=MagicMock(), response=fake_response)
    fake_response.raise_for_status = raise_for_status

    with patch("httpx.Client") as client_cls:
        client_cls.return_value.__enter__.return_value.post.return_value = fake_response
        r = extract_openrouter_llm(
            "<html></html>",
            {"base_url": "x", "api_key": "y", "model": "z", "timeout_secs": 30},
        )
    assert r.is_empty()
    assert any("openrouter-llm" in w for w in r.warnings)
