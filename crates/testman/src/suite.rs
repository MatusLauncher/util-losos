use crate::harness::{HarnessConfig, TestHarness};

pub enum TestResult {
    Pass,
    Fail(String),
    Timeout,
}

pub struct TestCase {
    pub name: String,
    pub run: Box<dyn Fn(&mut TestHarness) -> TestResult>,
}

pub struct TestSuite {
    cases: Vec<TestCase>,
}

impl TestSuite {
    pub fn new() -> Self {
        Self { cases: Vec::new() }
    }

    pub fn test(
        mut self,
        name: &str,
        f: impl Fn(&mut TestHarness) -> TestResult + 'static,
    ) -> Self {
        self.cases.push(TestCase {
            name: name.to_string(),
            run: Box::new(f),
        });
        self
    }

    pub fn run(self, config: HarnessConfig) -> miette::Result<TestReport> {
        let mut harness = TestHarness::start(config)?;
        let mut results = Vec::new();

        for case in self.cases {
            let result = (case.run)(&mut harness);
            results.push((case.name, result));
        }

        harness.shutdown();

        Ok(TestReport { results })
    }
}

impl Default for TestSuite {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TestReport {
    pub results: Vec<(String, TestResult)>,
}

impl TestReport {
    pub fn print(&self) {
        println!("\n--- Test Results ---");
        for (name, result) in &self.results {
            match result {
                TestResult::Pass => println!("  PASS  {}", name),
                TestResult::Fail(msg) => println!("  FAIL  {} — {}", name, msg),
                TestResult::Timeout => println!("  TIMEOUT  {}", name),
            }
        }
        let passed = self
            .results
            .iter()
            .filter(|(_, r)| matches!(r, TestResult::Pass))
            .count();
        let total = self.results.len();
        println!("--- {}/{} passed ---\n", passed, total);
    }

    pub fn has_failures(&self) -> bool {
        self.results
            .iter()
            .any(|(_, r)| !matches!(r, TestResult::Pass))
    }
}
