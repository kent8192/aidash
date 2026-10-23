//! The screened word lists Tenant names are drawn from. Every word is lowercase
//! ASCII, at most nine letters, and neutral: names reveal nothing about a Tenant
//! or its people, so nothing here may read as a judgement of either.

pub(crate) const ADJECTIVES: [&str; 128] = [
	"able", "agile", "airy", "amber", "ample", "azure", "balmy", "bold", "brave", "breezy",
	"bright", "brisk", "calm", "candid", "cheery", "civic", "clear", "clever", "cobalt", "coral",
	"cosmic", "crisp", "curious", "dapper", "daring", "deft", "dewy", "eager", "early", "earnest",
	"easy", "elated", "even", "fair", "fancy", "fast", "fresh", "frosty", "gentle", "giant",
	"glad", "golden", "grand", "green", "happy", "hardy", "hazel", "honest", "humble", "ivory",
	"jade", "jolly", "keen", "kind", "lively", "loyal", "lucid", "lucky", "lunar", "maple",
	"merry", "mighty", "mellow", "misty", "modest", "nimble", "noble", "olive", "open", "orange",
	"patient", "peaceful", "pearl", "plucky", "polite", "proud", "quick", "quiet", "radiant",
	"rapid", "ready", "rosy", "royal", "ruby", "rustic", "sage", "sandy", "serene", "sharp",
	"shiny", "silent", "silver", "simple", "sleek", "smooth", "snowy", "solar", "solid", "sonic",
	"spry", "starry", "steady", "stellar", "sturdy", "sunny", "swift", "tidy", "tranquil", "true",
	"upbeat", "valiant", "velvet", "vivid", "warm", "wavy", "wild", "windy", "wise", "witty",
	"woven", "young", "zesty", "zippy", "cozy", "lofty", "polar", "teal", "tawny",
];

pub(crate) const NOUNS: [&str; 128] = [
	"acorn", "alpaca", "anchor", "aspen", "badger", "bamboo", "beacon", "beaver", "birch", "bison",
	"brook", "canyon", "cedar", "cheetah", "cliff", "cloud", "comet", "condor", "coral", "cosmos",
	"coyote", "crane", "creek", "cypress", "delta", "dolphin", "dove", "dune", "eagle", "egret",
	"elk", "falcon", "fern", "finch", "fjord", "forest", "fox", "gazelle", "geyser", "glacier",
	"grove", "gull", "harbor", "hawk", "heron", "hill", "ibis", "island", "jaguar", "kestrel",
	"koala", "lagoon", "lark", "lemur", "lily", "lynx", "magpie", "maple", "marlin", "meadow",
	"meteor", "moose", "moss", "nebula", "newt", "oak", "ocean", "orca", "osprey", "otter", "owl",
	"panda", "panther", "pebble", "pelican", "penguin", "pine", "plover", "pond", "prairie",
	"puffin", "quail", "quartz", "rabbit", "raven", "reef", "ridge", "river", "robin", "salmon",
	"sequoia", "shore", "sparrow", "spruce", "stork", "summit", "swan", "tern", "thrush", "tiger",
	"trail", "trout", "tulip", "tundra", "turtle", "valley", "violet", "walrus", "willow", "wren",
	"yak", "zebra", "aurora", "bay", "breeze", "cove", "dawn", "ember", "galaxy", "harvest",
	"horizon", "lantern", "mesa", "orchid", "planet", "rain", "glade", "tide",
];

const fn longest(words: &[&str]) -> usize {
	let mut longest = 0;
	let mut i = 0;
	while i < words.len() {
		if words[i].len() > longest {
			longest = words[i].len();
		}
		i += 1;
	}
	longest
}
pub(crate) const LONGEST_ADJECTIVE: usize = longest(&ADJECTIVES);
pub(crate) const LONGEST_NOUN: usize = longest(&NOUNS);
