from crawl4ai_adapter.engines import ExtractResult


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
