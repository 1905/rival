# Rival.app developer shortcuts. Logic lives in app/scripts/*.py.
.PHONY: run install test soak

# Debug build, wrapped as "Rival (dev)", opened against your real ~/.rival.
run:
	python3 app/scripts/dev_bundle.py --build
	open -n app/.build/Rival.app

# Fresh release build (native arch) installed to /Applications/Rival.app.
# The old copy is moved to /tmp/trash, never deleted.
install:
	python3 app/scripts/install_local.py

# Unit tests, then the launch soak test: shows the dev app on screen for
# about a minute. Needs a GUI session; see app/scripts/soak_test.py.
test:
	cd app && swift test
	$(MAKE) soak

# The soak test alone (builds first).
soak:
	python3 app/scripts/soak_test.py
