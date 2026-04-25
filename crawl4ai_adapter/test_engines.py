import pytest
from crawl4ai_adapter.engines import ExtractResult, extract_trafilatura


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
