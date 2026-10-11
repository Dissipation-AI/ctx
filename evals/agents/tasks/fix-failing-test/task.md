The tests in this folder fail (`python3 -m pytest -q`, or `python3 -m unittest discover` style if
pytest isn't available: `python3 -c "import tests.test_cart as t; t.test_total(); t.test_discount()"`).
Find the bug in the code (not the tests), fix it, and show that the tests pass.
