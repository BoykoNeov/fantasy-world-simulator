//! Settings: every number of a world's configuration a person may change, each described in plain
//! words, with the range it may take and whether it may change while the world runs.
//!
//! A setting is named by its path in the configuration as TOML writes it ("world.provinces",
//! "modules.markets.trade.change"); `*` in a description's key stands for any one key of a table
//! (each good, each kind of troop). Values are read and written through the configuration's own
//! serialised form, so a setting needs no code of its own: only its line in [`GROUPS`]. Whatever
//! in the configuration is not a setting is listed in [`LEFT_OUT`] with the reason, and a test
//! checks that every value of every module is one or the other.
//!
//! This is for the window and for edits; nothing here runs during a day.

use crate::config::{Config, ModuleConfig};
use crate::culture::CultureParams;
use crate::research::ResearchParams;
use crate::diplomacy::{TreatyParams, WarParams};
use crate::economy::FoodParams;
use crate::markets::MarketParams;
use crate::pops::PopParams;
use crate::realms::RealmParams;
use crate::warfare::TroopParams;
use crate::world::{Size, WorldParams};
use toml::Value;

/// When a setting may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// Any day: from that day on, the world runs with the new value.
    Live,
    /// Only when a world is made: it shapes the map or the start, which are made once.
    NewWorld,
}

/// One setting, or one for each key of a table where the key has a `*`.
#[derive(Debug)]
pub struct Spec {
    pub key: &'static str,
    /// A few words, for the setting's line.
    pub label: &'static str,
    /// What it does and what changing it does, in plain words.
    pub help: &'static str,
    pub min: f64,
    pub max: f64,
    pub unit: &'static str,
    pub when: When,
}

/// The settings of one module (or of the world), shown together.
#[derive(Debug)]
pub struct Group {
    pub title: &'static str,
    /// Where its settings are, in the configuration: the group is there when this is.
    pub at: &'static str,
    pub help: &'static str,
    pub specs: &'static [Spec],
}

/// A part of a module that can be switched on or off, when a world is made.
#[derive(Debug)]
pub struct Section {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
}

const fn live(key: &'static str, label: &'static str, help: &'static str, min: f64, max: f64, unit: &'static str) -> Spec {
    Spec { key, label, help, min, max, unit, when: When::Live }
}

const fn made(key: &'static str, label: &'static str, help: &'static str, min: f64, max: f64, unit: &'static str) -> Spec {
    Spec { key, label, help, min, max, unit, when: When::NewWorld }
}

pub static GROUPS: &[Group] = &[
    Group {
        title: "World",
        at: "world",
        help: "The planet itself: its size, how finely it is mapped and how many provinces it has.",
        specs: &[
            made("world.n", "Map detail", "How finely the planet is divided into cells: it has 6 × this × this of them. More cells make a sharper, more detailed map, but one slower to make and to draw. The world sizes set this; at most the Huge world's, the largest run.", 16.0, 576.0, "cells a side"),
            made("world.provinces", "Provinces", "About how many provinces the land is divided into (islands may add a few). Fewer make bigger provinces and a faster world; more make smaller ones. The world sizes set this; at most the Huge world's, the most run.", 1.0, 20_000.0, ""),
            made("world.land_fraction", "Land", "Share of the planet's surface that is land; the rest is sea.", 0.02, 0.98, "share"),
            made("world.radius_km", "Planet radius", "How big the planet is. A bigger planet has bigger cells and provinces, longer roads and more land to farm, for the same map detail.", 500.0, 50_000.0, "km"),
        ],
    },
    Group {
        title: "Terrain",
        at: "modules.terrain_params",
        help: "The shape of the land and the sea floor. The last four shape the \"noise\" terrain only.",
        specs: &[
            made("modules.terrain_params.mountain_m", "Highest mountains", "How high the highest land rises. Height makes land colder, makes mountains and alpine land, and slows province borders.", 100.0, 12_000.0, "m"),
            made("modules.terrain_params.sea_depth_m", "Deepest sea", "How deep the deepest sea is. It matters when the sea level is changed: a shallow sea uncovers more land as it falls.", 100.0, 11_000.0, "m"),
            made("modules.terrain_params.highland_curve", "Lowland or highland", "How land is spread between low and high: above 1, most land is low plains with rare high peaks; at 1, every height is as common; below 1, broad high plateaus.", 0.2, 5.0, ""),
            made("modules.terrain_params.shelf_curve", "Coastal shelf", "How quickly the sea deepens away from the coast: below 1 it drops off fast; above 1, wide shallow seas along the coasts.", 0.2, 5.0, ""),
            made("modules.terrain_params.continent_scale", "Continents (more, smaller)", "How many continents fit around the planet: higher makes more, smaller continents and more islands; lower a few huge ones.", 0.3, 10.0, ""),
            made("modules.terrain_params.coast_warp", "Twisted coasts", "How strongly the continents' shapes are twisted: 0 gives round blobs; higher gives long arms, inlets and winding coasts.", 0.0, 3.0, ""),
            made("modules.terrain_params.detail", "Hills and islands", "How strong small bumps are against the continents' shape: more gives rougher land, more small islands and lakes.", 0.0, 2.0, ""),
            made("modules.terrain_params.ridges", "Mountain chains", "How strong the long mountain chains are: more gives more, higher ridges across the continents.", 0.0, 2.0, ""),
        ],
    },
    Group {
        title: "Climate",
        at: "modules.climate_params",
        help: "Yearly mean temperature and rain, by latitude, height and distance from the sea. The last two are the \"null\" climate's.",
        specs: &[
            made("modules.climate_params.equator_c", "Equator heat", "Yearly mean temperature at sea level on the equator. Raise it for a hotter world (more desert and jungle), lower it for an ice age.", -30.0, 60.0, "°C"),
            made("modules.climate_params.pole_drop_c", "Poles colder by", "How much colder the poles are than the equator. Small values give a mild world from end to end; large ones big ice caps.", 0.0, 150.0, "°C"),
            made("modules.climate_params.cooling_curve", "Where the cold begins", "How the cold spreads from the poles: above 1, warmth reaches far from the equator and the cold gathers near the poles; at 1 it cools evenly with latitude; below 1, the cold comes close to the equator.", 0.2, 5.0, ""),
            made("modules.climate_params.lapse_c_per_km", "Colder with height", "Degrees lost for each kilometre climbed. More makes mountains colder: snow and tundra lower down.", 0.0, 20.0, "°C a km"),
            made("modules.climate_params.temperature_noise_c", "Warm and cold patches", "Places are warmer or colder than their latitude says by up to this much, smoothly from place to place.", 0.0, 20.0, "°C"),
            made("modules.climate_params.base_rain_mm", "Least rain", "Rain a year on a coast that no rain belt reaches (land under about 250 mm is desert).", 0.0, 5000.0, "mm"),
            made("modules.climate_params.tropical_rain_mm", "Tropical rain", "Extra rain a year on the equator: the rainforest belt.", 0.0, 10_000.0, "mm"),
            made("modules.climate_params.tropical_width_deg", "Tropical belt width", "How far from the equator the tropical rain reaches.", 1.0, 60.0, "° latitude"),
            made("modules.climate_params.storm_rain_mm", "Storm-belt rain", "Extra rain a year in the mid-latitudes' belt of storms: the wet temperate lands.", 0.0, 10_000.0, "mm"),
            made("modules.climate_params.storm_lat_deg", "Storm belt latitude", "The latitude of the middle of the storm belt.", 0.0, 90.0, "°"),
            made("modules.climate_params.storm_width_deg", "Storm belt width", "How far the storm belt's rain reaches to either side.", 1.0, 60.0, "° latitude"),
            made("modules.climate_params.inland_rain", "Rain far inland", "Rain far from the sea, as a share of the rain on the coast: lower makes the hearts of continents drier (more steppe and desert).", 0.0, 1.0, "share"),
            made("modules.climate_params.inland_km", "Inland drying distance", "Over how far from the coast rain falls toward the inland level.", 10.0, 20_000.0, "km"),
            made("modules.climate_params.rain_noise", "Wet and dry patches", "Places get up to this share more or less rain than their belt gives, smoothly from place to place.", 0.0, 1.0, "share"),
            made("modules.climate_params.null_temperature_c", "Null climate: temperature", "With the \"null\" climate, the one temperature everywhere.", -30.0, 50.0, "°C"),
            made("modules.climate_params.null_rain_mm", "Null climate: rain", "With the \"null\" climate, the rain a year everywhere.", 0.0, 5000.0, "mm"),
        ],
    },
    Group {
        title: "Biomes",
        at: "modules.biome_params",
        help: "What grows where: each cell's biome comes from its yearly mean temperature, rain and height, by these limits, tried in this order.",
        specs: &[
            made("modules.biome_params.ice_below_c", "Ice cap below", "Land colder than this is ice cap: nothing grows there.", -60.0, 30.0, "°C"),
            made("modules.biome_params.alpine_above_m", "Alpine above", "Land higher than this is alpine: poor grazing only.", 0.0, 15_000.0, "m"),
            made("modules.biome_params.tundra_below_c", "Tundra below", "Land colder than this (but not ice) is tundra: poor grazing.", -60.0, 30.0, "°C"),
            made("modules.biome_params.desert_rain_mm", "Desert below", "Land with less rain than this a year is desert.", 0.0, 3000.0, "mm"),
            made("modules.biome_params.hot_desert_c", "Hot desert above", "Land warmer than this is desert with a little more rain too (the next setting), as heat dries it.", -30.0, 60.0, "°C"),
            made("modules.biome_params.hot_desert_rain_mm", "Hot desert below", "Hot land with less rain than this a year is desert.", 0.0, 3000.0, "mm"),
            made("modules.biome_params.boreal_below_c", "Boreal forest below", "Land colder than this (and not tundra or desert) is boreal forest: cold conifer woods.", -30.0, 40.0, "°C"),
            made("modules.biome_params.temperate_below_c", "Temperate below", "Land colder than this is temperate: grassland or forest by its rain. Warmer land is savanna or tropical forest.", -30.0, 50.0, "°C"),
            made("modules.biome_params.grassland_rain_mm", "Grassland below", "Temperate land with less rain than this a year is grassland; with more, temperate forest.", 0.0, 5000.0, "mm"),
            made("modules.biome_params.savanna_rain_mm", "Savanna below", "Warm land with less rain than this a year is savanna; with more, tropical forest.", 0.0, 10_000.0, "mm"),
        ],
    },
    Group {
        title: "Rivers",
        at: "modules.river_params",
        help: "Rain that doesn't dry off runs downhill and gathers into rivers.",
        specs: &[
            made("modules.river_params.evaporation_mm", "Evaporation", "Rain a year that dries off everywhere before it can run into rivers. More makes fewer, smaller rivers.", 0.0, 5000.0, "mm"),
            made("modules.river_params.evaporation_per_c", "More in the heat", "Extra rain lost to drying for each degree of yearly mean temperature above freezing: hot lands keep less of their rain.", 0.0, 500.0, "mm a °C"),
        ],
    },
    Group {
        title: "Farmland",
        at: "modules.farmland_params",
        help: "Food a square kilometre of each kind of land yields in an average year, in rations (a ration feeds one person for a day). This sets how many people the land can feed. Ice and sea yield none.",
        specs: &[
            made("modules.farmland_params.river_bonus", "River valleys", "Land a big river runs through yields this many times as much.", 0.0, 10.0, "times"),
            made("modules.farmland_params.tundra", "Tundra", "Food a km² of tundra yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.alpine", "Alpine", "Food a km² of alpine land yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.desert", "Desert", "Food a km² of desert yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.boreal_forest", "Boreal forest", "Food a km² of boreal forest yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.temperate_forest", "Temperate forest", "Food a km² of temperate forest yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.grassland", "Grassland", "Food a km² of grassland yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.savanna", "Savanna", "Food a km² of savanna yields a year.", 0.0, 20_000.0, "rations"),
            made("modules.farmland_params.tropical_forest", "Tropical forest", "Food a km² of tropical forest yields a year.", 0.0, 20_000.0, "rations"),
        ],
    },
    Group {
        title: "Provinces",
        at: "modules.province_params",
        help: "How the land is divided. Provinces grow outward from their centres until they meet; each step of growth has a cost, and where steps cost more, growth slows and borders form. How many provinces there are is under World.",
        specs: &[
            made("modules.province_params.poor_land_weight", "Centres on poor land", "How readily province centres fall on poor land: near 0, they crowd onto fertile land (many small provinces there, a few huge ones in desert and tundra); higher, they spread by area and provinces are more alike in size.", 0.0, 100.0, ""),
            made("modules.province_params.plain_step", "Cost of open land", "What a step across flat, poor land costs. The costs below are measured against it: raise it and mountains, rivers and farmland matter less.", 1.0, 100_000.0, ""),
            made("modules.province_params.fertile_step", "Smaller on farmland", "Extra cost of a step through the most fertile land: higher makes provinces smaller where the land is rich, so farmland is shared out more evenly than area.", 0.0, 100_000.0, ""),
            made("modules.province_params.climb_step", "Borders follow mountains", "Extra cost for each metre a step climbs or descends: higher makes borders follow ridges and mountain ranges more closely.", 0.0, 1000.0, "a metre"),
            made("modules.province_params.river_crossing", "Borders follow rivers", "Extra cost of stepping onto or off a big river: higher makes big rivers into borders more often.", 0.0, 1_000_000.0, ""),
            made("modules.province_params.roughness", "Wandering borders", "How much the cost of crossing land varies from place to place: 0 gives straighter borders and rounder provinces; higher makes borders meander and provinces irregular.", 0.0, 5.0, ""),
        ],
    },
    Group {
        title: "Economics: food",
        at: "modules.economics",
        help: "The \"food\" economy: harvests fill each province's food store, people eat from it, and stores spoil.",
        specs: &[
            live("modules.economics.spoilage_per_year", "Spoiling", "Share of stored food that rots over a year.", 0.0, 1.0, "share"),
            made("modules.economics.initial_stock_days", "Starting stores", "Food each province has stored at the start, in days of an average harvest.", 0.0, 3650.0, "days"),
            live("modules.economics.harvest_variability", "Good and bad years", "How much a region's harvest swings from year to year: 0 never, 1 from nothing to double. Bad years bring famine.", 0.0, 2.0, ""),
            live("modules.economics.seasonality", "Harvest season", "How strongly the harvest gathers in late summer far from the equator: 0 the same every month; 1 nearly all in a few months.", 0.0, 1.0, ""),
            live("modules.economics.harvest_peak", "Harvest time", "When the northern harvest peaks, as a share of the year (0.62 is mid-August); the south's comes half a year later.", 0.0, 1.0, "of the year"),
        ],
    },
    Group {
        title: "Population",
        at: "modules.population",
        help: "People are born, die of age, and starve when there isn't food.",
        specs: &[
            live("modules.population.births_per_year", "Births", "Children born a year, for each person. Births above deaths make the people grow while there is food.", 0.0, 0.2, "a person"),
            live("modules.population.deaths_per_year", "Deaths", "People who die of age and illness a year, for each person.", 0.0, 0.2, "a person"),
            live("modules.population.starvation_deaths", "Starving", "Share of the unfed who die for each month (30 days) they go hungry.", 0.0, 1.0, "share"),
            made("modules.population.initial_fill", "Starting people", "People at the start, as a share of what an average harvest can feed: above 1, the world starts in famine.", 0.0, 3.0, "share"),
        ],
    },
    Group {
        title: "Economics: markets",
        at: "modules.markets",
        help: "The \"markets\" economy: workers make goods from land and people, each province prices its goods by its stores, and goods are traded between neighbours for coins.",
        specs: &[
            live("modules.markets.working_share", "Workers", "Share of the people who work; the rest are children, old or at home.", 0.0, 1.0, "share"),
            made("modules.markets.coins_per_person", "Starting coins", "Coins each person has at the start (the province holds them). Coins are never made or lost after that; trade and taxes move them.", 0.0, 10_000.0, "coins"),
            live("modules.markets.job_change", "Changing jobs", "How far workers move toward the best-paying work each month: 0 never, 1 all at once (jumpy).", 0.0, 1.0, "share"),
            live("modules.markets.idle_worth", "Worth working", "Coins a year that work must be worth for a worker to do it rather than rest (a worker's year at base prices is worth about 200).", 0.0, 10_000.0, "coins"),
            live("modules.markets.price_response", "Prices follow stores", "How strongly a price answers its stores: base price × (stores wanted ÷ stores held) to this power. 0: prices never change; higher: a shortage makes prices soar.", 0.0, 5.0, ""),
            live("modules.markets.price_change", "Price speed", "How far a price moves toward that each month: 0 never, 1 at once.", 0.0, 1.0, "share"),
            live("modules.markets.price_range", "Price limits", "Prices never go below the base price ÷ this or above it × this.", 1.0, 1000.0, "times"),
            made("modules.markets.initial_stock_months", "Starting stores", "Stores of each good at the start, in months of what the province makes.", 0.0, 120.0, "months"),
            live("modules.markets.harvest_variability", "Good and bad years", "How much a region's harvest swings from year to year: 0 never, 1 from nothing to double.", 0.0, 2.0, ""),
            live("modules.markets.seasonality", "Harvest season", "How strongly the harvest gathers in late summer far from the equator: 0 the same every month; 1 nearly all in a few months.", 0.0, 1.0, ""),
            live("modules.markets.harvest_peak", "Harvest time", "When the northern harvest peaks, as a share of the year (0.62 is mid-August); the south's comes half a year later.", 0.0, 1.0, "of the year"),
            live("modules.markets.tools_per_worker", "Tools a worker", "Tools a fully equipped worker holds.", 0.0, 100.0, "tools"),
            live("modules.markets.tool_boost", "Tools help", "How much more a fully equipped worker makes: 0.5 is half as much again.", 0.0, 10.0, ""),
            live("modules.markets.groups.*.need_per_person", "wanted a person", "What each person wants of these goods a year (food: a ration a day is 365); the stores wanted, and so prices, are worked out from it.", 0.0, 100_000.0, "a year"),
            live("modules.markets.groups.*.reserve_months", "stores wanted", "Stores people want to keep, in months of what they want; prices rise as stores fall below this.", 0.0, 120.0, "months"),
            live("modules.markets.goods.*.per_worker", "made per worker", "What one worker makes of this good in a year while land is plentiful (it falls as the land fills up).", 0.0, 1_000_000.0, "a year"),
            live("modules.markets.goods.*.use_per_person", "used per person", "What each person uses up of it a year (food is eaten through Population instead).", 0.0, 10_000.0, "a year"),
            live("modules.markets.goods.*.reserve_months", "stores wanted", "Stores wanted, in months of use (goods priced in a group use the group's).", 0.0, 120.0, "months"),
            live("modules.markets.goods.*.base_price", "base price", "Coins for one unit when stores are as wanted: the middle of its prices.", 0.0001, 100_000.0, "coins"),
            live("modules.markets.goods.*.lost_per_year", "lost a year", "Share of the store that rots or wears out over a year.", 0.0, 1.0, "share"),
            live("modules.markets.goods.*.inputs.*", "taken per unit", "How much of another good one unit of this one takes to make.", 0.0, 1000.0, ""),
            live("modules.markets.trade.change", "Trade speed", "How much of a price gap between neighbours (beyond the carrying cost) trade closes in a month: the damper on trade.", 0.0, 1.0, "share"),
            live("modules.markets.trade.most_sold", "Most sold a month", "The most of a store a province sells in a month.", 0.0, 1.0, "share"),
            live("modules.markets.trade.most_spent", "Most spent a month", "The most of its coins a province spends on goods in a month.", 0.0, 1.0, "share"),
            live("modules.markets.trade.sea_share", "Sea is cheaper", "A day of carrying at sea costs this share of a day overland (0.1: ten times cheaper).", 0.0, 10.0, "share"),
            live("modules.markets.trade.money_response", "Prices follow money", "How strongly prices follow the coins about: where coins pile up everything is dearer, by the coins a person against the world's to this power, so the coins flow back out. 0: not at all.", 0.0, 5.0, ""),
            live("modules.markets.trade.money_range", "Money price limits", "That dearness stays within 1/this and this times.", 1.0, 100.0, "times"),
            live("modules.markets.trade.toll", "Border tolls", "Goods coming into another realm pay this share of the seller's price as a toll, on top of the price and the carrying, into that realm's treasury. Carrying food a day already costs about a fifth of its price, so a toll hardly touches food and bites on cloth and tools, worth much for their weight. Raise it and less trade crosses borders but treasuries fill; 0: no tolls. None into land nobody owns, an enemy occupies or that has risen up, nor within a realm.", 0.0, 5.0, "share"),
            live("modules.markets.trade.war_stops", "War stops trade", "Share of the trade between two realms at war that the war stops: 1, none goes; 0, they trade as in peace. Land an enemy occupies trades as the occupier's.", 0.0, 1.0, "share"),
            live("modules.markets.trade.siege_stops", "Siege stops trade", "Share of the trade into and out of a province enemies besiege that the siege stops: 1, none goes; 0, as if there were no siege. Only armies' \"sieges in depth\" mark provinces besieged, so without them this changes nothing.", 0.0, 1.0, "share"),
            live("modules.markets.trade.carriage.*", "carrying cost", "Coins to carry one unit of this good one day overland. Higher keeps it local.", 0.0, 1000.0, "coins a day"),
        ],
    },
    Group {
        title: "Realms",
        at: "modules.realms",
        help: "Countries: how many start and how they are laid out, the taxes they raise, and unrest and uprisings in their provinces. The settled land is shared between three ways to start by the first three weights (any scale): realms over settled land, city-states, and a frontier with unclaimed land between realms.",
        specs: &[
            made("modules.realms.settled", "Realms over settled land", "Weight of realms that start over settled land, about \"provinces per realm\" each.", 0.0, 100.0, "weight"),
            made("modules.realms.city_states", "City-states", "Weight of many small city-states, of one up to \"largest city-state\" provinces each.", 0.0, 100.0, "weight"),
            made("modules.realms.frontier", "Frontier", "Weight of realms that start with unclaimed land between them, which they claim as time goes on.", 0.0, 100.0, "weight"),
            made("modules.realms.provinces_per_realm", "Provinces per realm", "About how many provinces a realm over settled land (or on the frontier) starts with: fewer makes more, smaller realms. Used when \"number of realms\" is 0.", 1.0, 10_000.0, "provinces"),
            made("modules.realms.realm_count", "Number of realms", "About how many realms start over settled land and on the frontier together, whatever the province count (city-states come on top, and an island or other land none of them can reach starts a realm of its own). 0: they are made \"provinces per realm\" each instead, so more provinces make more realms.", 0.0, 10_000.0, "realms"),
            made("modules.realms.city_state_max", "Largest city-state", "Each city-state starts with from one up to this many provinces.", 1.0, 100.0, "provinces"),
            made("modules.realms.unclaimed_share", "Frontier left unclaimed", "On the frontier, the share of each realm's land left unclaimed at the start.", 0.0, 0.99, "share"),
            live("modules.realms.claim_per_month", "Claiming land", "Each month, the chance that a settled province nobody owns is claimed by a realm next to it.", 0.0, 1.0, "chance"),
            live("modules.realms.taxes.per_month", "Taxes", "Share of a province's coins taken into its realm's treasury each month (in a world with coins).", 0.0, 1.0, "share"),
            live("modules.realms.taxes.spent_per_month", "Spending", "Share of its treasury a realm spends each month, back into its provinces.", 0.0, 1.0, "share"),
            live("modules.realms.taxes.capital_weight", "Capital's share", "A realm spends into its provinces by their people, the capital's counted this many times. 1: like any province; more makes the capital richer, and rich capitals are hard to take, so a rich city is rarely a war's prize.", 1.0, 1000.0, "times"),
            live("modules.realms.unrest.half_life_years", "Unrest fading", "Years for unrest to fall by half while nothing stirs it.", 0.01, 100.0, "years"),
            live("modules.realms.unrest.famine", "Unrest from famine", "Unrest a month of famine adds (a province rises up at 1000).", 0.0, 1000.0, ""),
            live("modules.realms.unrest.famine_share", "A famine month", "A month counts as full famine when this share of the people starved, and in proportion for fewer.", 0.0001, 1.0, "share"),
            live("modules.realms.unrest.occupied", "Unrest from occupation", "Unrest each month of enemy occupation adds.", 0.0, 1000.0, ""),
            live("modules.realms.unrest.child_heir", "Unrest from a child ruler", "Unrest a child coming to the throne adds in every province of the realm.", 0.0, 1000.0, ""),
            live("modules.realms.unrest.claimant", "Unrest from a contested throne", "Unrest a claimant coming to the throne, with no heir, adds in every province of the realm.", 0.0, 1000.0, ""),
            live("modules.realms.unrest.join", "Joining an uprising", "A province next to an uprising in its own realm joins it when its unrest is this or more. At least \"uprising dies down\".", 0.0, 1000.0, ""),
            live("modules.realms.unrest.calm", "Uprising dies down", "An uprising ends in a province once its unrest falls below this. At most \"joining an uprising\".", 0.0, 1000.0, ""),
            live("modules.realms.unrest.breakaway", "Breaking away", "Provinces in uprising that hold together, this many or more, break away as a new realm.", 1.0, 1000.0, "provinces"),
        ],
    },
    Group {
        title: "Rulers",
        at: "modules.characters",
        help: "Every realm has a ruler and, often, an heir. Rulers age and die, and how the throne passes stirs unrest.",
        specs: &[
            live("modules.characters.births_per_year", "Heirs born", "Each year, the chance that a ruler with no heir, aged 16 to 50, has a child, who becomes heir.", 0.0, 1.0, "chance"),
            live("modules.characters.kin_per_year", "Kin named heir", "Each year, the chance that a ruler with no heir names a grown kinsman heir.", 0.0, 1.0, "chance"),
            live("modules.characters.mortality", "Deadliness", "Multiplies everyone's chance of dying: 1 as the life table has it, 2 twice as deadly.", 0.0, 20.0, "times"),
        ],
    },
    Group {
        title: "Diplomacy",
        at: "modules.diplomacy",
        help: "Wars, peace and truces. With \"realms choose war and peace\" on, realms think of each other, want things, declare war and make peace themselves; the settings under \"treaties\" are theirs. Opinions go from -100 to +100.",
        specs: &[
            live("modules.diplomacy.truce_years", "Truce", "Years a truce lasts after a peace, while the two can't go to war again.", 0.0, 100.0, "years"),
            live("modules.diplomacy.rebel_war_years", "Rebels win after", "A rebellion's war still going after this many years ends with the rebels free.", 0.0, 100.0, "years"),
            live("modules.diplomacy.treaties.war_chance", "War chance", "The monthly chance a realm declares war on its best target at a score of 1; the score multiplies what it wants, how much stronger it is, its ruler's warlike nature and its opinion of the target.", 0.0, 1.0, "chance"),
            live("modules.diplomacy.treaties.most_score", "Highest score", "Scores above this count as this: the most eager a realm can get.", 0.0, 1000.0, ""),
            live("modules.diplomacy.treaties.hunger_line", "Wanting food", "A realm wants food (a cause for war) once this share of its people starved over the last twelve months.", 0.0, 1.0, "share"),
            live("modules.diplomacy.treaties.least_strength", "Strength to attack", "A realm declares war only on one it is at least this times as strong as (0.8: it may attack one a little stronger).", 0.0, 10.0, "times"),
            live("modules.diplomacy.treaties.second_war", "A second war", "A realm already at war declares another only when at least this times as strong as all its enemies together, the new one included.", 0.0, 20.0, "times"),
            live("modules.diplomacy.treaties.opinion_start", "Opinion on meeting", "What two realms think of each other when they first meet.", -100.0, 100.0, ""),
            live("modules.diplomacy.treaties.opinion_rebels", "Rebels' opinion", "What rebels who broke away and the realm they left think of each other.", -100.0, 100.0, ""),
            live("modules.diplomacy.treaties.peace_rise", "Warming in peace", "At peace and outside a truce, each opinion rises this much a month, up to \"peace's ceiling\".", 0.0, 10.0, "a month"),
            live("modules.diplomacy.treaties.peace_ceiling", "Peace's ceiling", "Long peace warms opinions only up to this.", 0.0, 100.0, ""),
            live("modules.diplomacy.treaties.rival_ratio", "Rivals", "Neighbours within this ratio of each other's strength are rivals (1.5: neither is more than half as strong again as the other).", 1.0, 10.0, "times"),
            live("modules.diplomacy.treaties.rival_fall", "Rivalry", "Rivals think this much less of each other each month.", 0.0, 10.0, "a month"),
            live("modules.diplomacy.treaties.attacked_fall", "Being attacked", "When war is declared, the realm attacked thinks this much less of the attacker.", 0.0, 200.0, ""),
            live("modules.diplomacy.treaties.declarer_fall", "Attacking", "When war is declared, the attacker thinks this much less of the realm it attacks.", 0.0, 200.0, ""),
            live("modules.diplomacy.treaties.battle_fall", "Battles", "Each battle between two realms: each thinks this much less of the other.", 0.0, 200.0, ""),
            live("modules.diplomacy.treaties.occupied_fall", "Occupation", "Each province occupied: its owner thinks this much less of the occupier.", 0.0, 200.0, ""),
            live("modules.diplomacy.treaties.land_lost_fall", "Land lost", "Land lost at a peace: the loser thinks this much less of the winner.", 0.0, 200.0, ""),
            live("modules.diplomacy.treaties.worn", "Worn out at", "A side is worn out at this much weariness, and gives in or seeks peace.", 0.01, 1000.0, ""),
            live("modules.diplomacy.treaties.occupied_weariness", "Weariness: occupation", "Weariness while all of a side's people live under enemy occupation, and in proportion for fewer.", 0.0, 1000.0, ""),
            live("modules.diplomacy.treaties.year_weariness", "Weariness: years", "Weariness for each year at war.", 0.0, 1000.0, "a year"),
            live("modules.diplomacy.treaties.battle_weariness", "Weariness: battles lost", "Weariness for each battle lost.", 0.0, 1000.0, "a battle"),
            live("modules.diplomacy.treaties.dead_weariness", "Weariness: the dead", "Weariness for soldiers lost as many as \"the war-time share\" of its people, and in proportion.", 0.0, 1000.0, ""),
            live("modules.diplomacy.treaties.war_share", "The war-time share", "The share of its people whose loss as soldiers counts the whole of \"weariness: the dead\".", 0.0001, 1.0, "share"),
            live("modules.diplomacy.treaties.unrest_weariness", "Weariness: unrest", "Weariness for a whole realm on the brink of uprising, by its average unrest.", 0.0, 1000.0, ""),
            live("modules.diplomacy.treaties.held_target_weariness", "Peace once the prize is held", "An attacker holding what it went to war for makes peace once the defender's weariness reaches this.", 0.0, 1000.0, ""),
            live("modules.diplomacy.treaties.tie", "A draw", "Both sides worn out within this of each other: nobody wins.", 0.0, 1000.0, ""),
            live("modules.diplomacy.treaties.trade_opinion", "Warming by trade", "At peace and outside a truce, a realm thinks up to this much better a month of a realm it trades with, on top of \"warming in peace\" and up to \"peace's ceiling\". Higher: trading partners become friends faster and rarely fight. 0: trade doesn't change opinions.", 0.0, 10.0, "a month"),
            live("modules.diplomacy.treaties.trade_full", "Trade that counts fully", "A month's trade with another realm worth this share of the coins in a realm's own land warms it the whole \"warming by trade\"; less trade, in proportion. Lower: even a little trade counts fully.", 0.0001, 1.0, "share"),
        ],
    },
    Group {
        title: "Armies",
        at: "modules.warfare",
        help: "Provinces raise soldiers out of their people: a few guards in peace, many more in war, who march under their realm's general, fight battles and besiege provinces. Soldiers eat where they stand, and the hungry desert.",
        specs: &[
            live("modules.warfare.peace_share", "Soldiers in peace", "Share of a province's people under arms in peace: guards who stay home.", 0.0, 1.0, "share"),
            live("modules.warfare.war_share", "Soldiers in war", "Share under arms while its realm is at war; those over the peace share march.", 0.0, 1.0, "share"),
            live("modules.warfare.months_to_raise", "Months to raise", "A province raises at most the war-time number ÷ this each month.", 0.01, 120.0, "months"),
            live("modules.warfare.disband_slack", "Before going home", "Soldiers go home only once a province has this share more than it wants, so a few deaths or births don't send them back and forth.", 0.0, 10.0, "share"),
            live("modules.warfare.raise_below", "Before raising more", "A province raises more only once it has less than this share of the soldiers it wants.", 0.0, 1.0, "share"),
            live("modules.warfare.desert_share", "Deserting", "Share of a month's unfed soldiers who desert home.", 0.0, 1.0, "share"),
            live("modules.warfare.deaths_per_year", "Soldiers' deaths", "Soldiers who die of age and illness a year, for each soldier.", 0.0, 1.0, "a soldier"),
            live("modules.warfare.starvation_deaths", "Soldiers starving", "Share of the unfed who stay that die, for each month of hunger.", 0.0, 1.0, "share"),
            live("modules.warfare.bounty", "Bounty", "Coins a realm pays a province for each soldier raised there, where the world keeps coins.", 0.0, 10_000.0, "coins"),
            live("modules.warfare.gather_share", "Gathering", "A province where an army gathers takes its realm's soldiers up to this share of its people, so its food isn't stretched too far.", 0.0, 1.0, "share"),
            live("modules.warfare.outmatch", "Attack when stronger by", "The general attacks when the strength it sends is this many times the enemy's standing there (counting their edge at home).", 0.01, 20.0, "times"),
            live("modules.warfare.battle_rate", "Bloodiness", "Each day of battle, a side loses this share of the other side's strength.", 0.0, 1.0, "share"),
            live("modules.warfare.luck", "Luck", "Each side's blows each day are up to this share stronger or weaker, at random.", 0.0, 1.0, "share"),
            live("modules.warfare.home_edge", "Home edge", "The side on its own land deals this share more.", 0.0, 10.0, "share"),
            live("modules.warfare.break_share", "Breaking", "A side breaks and falls back once it has lost this share of the soldiers it fought with.", 0.01, 1.0, "share"),
            live("modules.warfare.rest_days", "Rest after breaking", "Days a company that broke isn't sent to attack.", 0.0, 3650.0, "days"),
            live("modules.warfare.siege_base_days", "Siege days", "A siege takes about this × (1 + the province's people ÷ the people of the world's middle province) days: about a month for a small province, two for a middling one.", 0.1, 3650.0, "days"),
            live("modules.warfare.capital_siege", "Capitals hold out", "A capital's siege takes this many times as long as another province's. 1: no longer; more makes capitals hold out.", 0.01, 100.0, "times"),
            live("modules.warfare.siege_max_days", "Longest siege", "No siege takes longer than this.", 1.0, 36_500.0, "days"),
            live("modules.warfare.free_siege", "Freeing one's own", "Freeing a province of one's own takes this share of a siege's time.", 0.0, 1.0, "share"),
            live("modules.warfare.siege_min_share", "Besiegers needed", "Besiegers fewer than this share of the province's people make no progress.", 0.0, 1.0, "share"),
            live("modules.warfare.kinds.*.weight", "share raised", "How much of a province's raising goes to this kind of troop, times the share of its land that suits it.", 0.0, 100.0, "weight"),
            live("modules.warfare.kinds.*.per_soldier", "goods a soldier", "What each soldier of this kind uses up of their good when raised (tools for heavy infantry, timber for ships). Above 0 for a kind that needs a good.", 0.0, 1000.0, ""),
            live("modules.warfare.kinds.*.strength", "strength", "A soldier's strength in battle: what they deal, and what it takes to kill them.", 0.01, 1000.0, ""),
            live("modules.warfare.kinds.*.speed", "speed", "How fast they go: a road's days are divided by this. A group goes at its slowest.", 0.01, 100.0, "times"),
            live("modules.warfare.kinds.*.carry", "carries", "For ships: the soldiers each crewman takes across the sea.", 0.0, 1000.0, "soldiers"),
        ],
    },
    // The first wave of modules built side by side (Oct 17): a block of groups each. Whoever builds
    // a module adds its settings inside its own block only.
    // --- Logistics and support (the four parts of Warfare "campaigns") ---
    Group {
        title: "Supply lines and foraging",
        at: "modules.warfare.supply",
        help: "Soldiers near their realm's land are fed from home along supply lines; further out they forage, taking food from the land they stand in. On an enemy's land foragers take before the people there.",
        specs: &[
            live("modules.warfare.supply.reach_ways", "Supply line reach", "Companies at most this many provinces beyond their realm's own land (counted in borders crossed, the fewest it takes) are fed from their home province along a supply line; further out they forage from the land they stand in. More: armies stay fed deeper in enemy land. 0: every company away from its own land forages. Counted in provinces, not days, so it means the same on a world of big provinces as of small ones.", 0.0, 20.0, "provinces"),
            live("modules.warfare.supply.carry_loss", "Lost on the way", "Share of the food sent along a supply line lost for each day of the way (spoiled, or eaten by the carriers). Higher makes long supply lines cost the home provinces more food.", 0.0, 0.5, "share a day"),
            live("modules.warfare.supply.forage_take", "Foragers take", "Foragers take this many times what they eat from the land they stand in; the rest is wasted. Higher strips the land, and its people's food, faster.", 1.0, 10.0, "times"),
            live("modules.warfare.supply.camp_disease", "Camp disease", "Share of a company's soldiers who die of camp disease each month while away from their own land. Higher makes long campaigns costly.", 0.0, 1.0, "share a month"),
        ],
    },
    Group {
        title: "Winter",
        at: "modules.warfare.seasons",
        help: "Far from the equator, winter slows marches, kills soldiers in camp and makes foraging poor, and no new attacks are begun in deep winter. Each effect is given for the deepest winter, and is less in a milder one.",
        specs: &[
            live("modules.warfare.seasons.start_lat", "Winter begins at", "Degrees of latitude from the equator where winter begins; nearer the equator there is none.", 0.0, 90.0, "°"),
            live("modules.warfare.seasons.full_span", "Full winter further by", "Degrees further from the equator where winter is as deep as it gets. Smaller: winter grows harsh quickly past where it begins.", 0.1, 90.0, "°"),
            live("modules.warfare.seasons.deepest", "Deepest winter", "When in the year winter is deepest in the north, as a share of the year from its first day (0.05: about the 18th day); in the south half a year later. Winter lasts half the year around it.", 0.0, 1.0, "share of the year"),
            live("modules.warfare.seasons.road_slow", "Winter roads", "In the deepest winter, marches take this many times as long. 1: winter doesn't slow them.", 1.0, 10.0, "times"),
            live("modules.warfare.seasons.deaths", "Winter deaths", "In the deepest winter, the share of soldiers away from their own land who die of cold each month.", 0.0, 1.0, "share a month"),
            live("modules.warfare.seasons.forage_loss", "Winter foraging", "In the deepest winter, foragers find this share less food (and go hungry).", 0.0, 1.0, "share"),
            live("modules.warfare.seasons.no_attack", "No attacks in winter above", "No new attack is begun on a province whose winter is deeper than this, from 0 (none) to 1 (the deepest): 0 stops attacks in any winter, 1 lets them go on all year. Armies already marching go on.", 0.0, 1.0, ""),
        ],
    },
    Group {
        title: "Soldiers' pay",
        at: "modules.warfare.pay",
        help: "Where the world keeps coins, realms pay all their soldiers, guards too, from the treasury every month, into each soldier's home province; unpaid soldiers desert. Without coins (Economics v0) soldiers serve unpaid.",
        specs: &[
            live("modules.warfare.pay.wage", "Wage", "Coins a realm pays each soldier a month. People start with 20 coins each and pay about 1% of theirs a month in taxes, so at 1 coin peace's few guards cost a realm little and a war's armies about a fifth of its taxes.", 0.0, 10_000.0, "coins a month"),
            live("modules.warfare.pay.unpaid_desert", "Unpaid desert", "Share of the soldiers a realm couldn't pay in a month who desert home.", 0.0, 1.0, "share"),
        ],
    },
    Group {
        title: "Sieges",
        at: "modules.warfare.sieges",
        help: "A besieged province raises no soldiers, its trade is cut (see \"siege stops trade\" under markets) and its stores stripped, and it falls faster once its food runs low. Besiegers eat before the people there.",
        specs: &[
            live("modules.warfare.sieges.raising", "Raising under siege", "A besieged province raises this share of the soldiers it otherwise would. 0: none, so a besieged city can't refill its garrison.", 0.0, 1.0, "share"),
            live("modules.warfare.sieges.strip", "Stores stripped", "Share of a besieged province's food store the besiegers take each month.", 0.0, 1.0, "share a month"),
            live("modules.warfare.sieges.starving_days", "Starving below", "A besieged province whose food would last its people fewer than this many days is starving: its siege goes faster.", 0.0, 3650.0, "days"),
            live("modules.warfare.sieges.starving_speed", "Starving sieges", "Each day a starving province is besieged counts this many days of its siege. 1: hunger doesn't hasten a fall.", 1.0, 100.0, "times"),
            live("modules.warfare.sieges.plunder", "Plunder", "Where the world keeps coins: the share of a province's coins the realm that takes it by siege carries off into its treasury.", 0.0, 1.0, "share"),
        ],
    },
    Group {
        title: "Armies acting together",
        at: "modules.warfare.massing",
        help: "Soldiers from many provinces march to gather against a target: the ones already there set off as soon as together they outmatch the defence, without waiting for the rest, and a gathering that drags on is given up and planned again.",
        specs: &[
            live("modules.warfare.massing.join_days", "Longest wait to gather", "A group gathering against a target that hasn't set off this many days after it was formed is given up and planned again. Fewer: armies give up sooner on slow gatherings.", 0.0, 3650.0, "days"),
            live("modules.warfare.massing.mass_share", "Most sent at one target", "No group takes more than this share of its realm's free soldiers (those not in another group), so one target can't draw in a whole realm's armies. 1: no limit.", 0.01, 1.0, "share"),
        ],
    },
    // --- end of logistics ---
    // --- Research ---
    Group {
        title: "Research",
        at: "modules.research",
        help: "Realms study and learn techs from a tree, one at a time, each once they know the techs it needs. Bigger realms and able rulers learn faster, and a tech that neighbours and trade partners know is cheaper. In a world with coins, scholars are paid wages from the treasury. For now knowledge does nothing else: what techs do comes later.",
        specs: &[
            live("modules.research.speed", "Study speed", "Years of a tech's cost that a middle-sized realm (by people, against the other living realms) with an ordinary ruler studies in a year. Raise it and every realm learns faster; at 0 nobody learns anything.", 0.0, 10.0, "years a year"),
            live("modules.research.size_power", "Bigger realms learn faster", "A realm studies as fast as its people against the middle realm's, to this power: 0, every realm alike; 0.5, a realm with four times the people studies twice as fast; 1, in proportion to its people.", 0.0, 1.0, "power"),
            live("modules.research.size_cap", "Fastest big realm", "However big, a realm studies at most this many times as fast as the middle realm. Lower it to keep the biggest realms from racing ahead; 1 makes no realm faster than the middle one.", 1.0, 100.0, "times"),
            live("modules.research.ruler_effect", "Rulers' ability", "A ruler of the highest ability studies this share faster, one of the lowest this share slower, an ordinary one (or none) neither. 0: rulers make no difference.", 0.0, 1.0, "share"),
            live("modules.research.neighbour_discount", "Learning from neighbours", "A tech that every neighbour (and trade partner) knows costs this share less; one that some of them know, in proportion. Higher: knowledge spreads faster and realms keep closer together. 0: every realm learns alone.", 0.0, 0.95, "share"),
            live("modules.research.trade_weight", "Learning from trade", "How much a trade partner (a realm traded with across the border last month) counts against a neighbour's 1 in that share. 0: trade passes on no knowledge. A world without trade has no partners.", 0.0, 10.0, "weight"),
            live("modules.research.scholar_share", "Scholars", "In a world with coins, the share of the people who are scholars and are paid wages. More scholars cost the treasury more; they don't study faster (study speed and the realm's size set that). 0: no wages, study is free.", 0.0, 0.1, "share"),
            live("modules.research.wage", "Scholars' wage", "Coins a scholar is paid a year from the treasury, a twelfth each month. Unpaid scholars don't study, so a realm short of coins learns slower: wages are paid after the realm's own spending, sharing what is left with its other lesser payments (such as soldiers' bounties). 0: study is free.", 0.0, 10_000.0, "coins a year"),
            live("modules.research.techs.*.years", "years of study", "What this tech costs: years of study for a middle-sized realm with an ordinary ruler, before neighbours make it cheaper.", 0.1, 1000.0, "years"),
        ],
    },
    // --- end of research ---
    // --- Culture ---
    Group {
        title: "Culture",
        at: "modules.culture",
        help: "Peoples spread over regions when the world is made; over the years people take up the culture of their neighbours and rulers, and cut-off groups become cultures of their own.",
        specs: &[],
    },
    // --- end of culture ---
];

/// Parts of modules that can be switched on or off when a world is made.
pub static SECTIONS: &[Section] = &[
    Section { key: "modules.markets.trade", label: "Trade between provinces", help: "Goods go from a province to a neighbour where they are dearer, for coins. Off: every province lives on what it makes." },
    Section { key: "modules.realms.unrest", label: "Unrest and uprisings", help: "Famine, occupation and troubled successions make provinces restless; restless provinces rise up and may break away." },
    Section { key: "modules.diplomacy.treaties", label: "Realms choose war and peace", help: "Realms think of each other, declare war and make peace themselves (Diplomacy \"treaties\"; needs armies). Off: wars come only from your pokes and rebellions." },
    // The first wave of modules built side by side (Oct 17): a block each.
    // --- Logistics and support ---
    Section { key: "modules.warfare.supply", label: "Supply lines and foraging", help: "Soldiers are fed from home along supply lines, or forage beyond them. Off: soldiers eat where they stand, as they share food with the people there." },
    Section { key: "modules.warfare.seasons", label: "Winter", help: "Winter slows armies, kills soldiers in camp and stops new attacks far from the equator. Off: war is the same all year." },
    Section { key: "modules.warfare.pay", label: "Soldiers' pay", help: "Realms pay their soldiers wages from the treasury, where the world keeps coins. Off: soldiers serve unpaid." },
    Section { key: "modules.warfare.massing", label: "Armies acting together", help: "Soldiers gathered against a target set off once together they outmatch the defence, without waiting for stragglers; slow gatherings are given up. Off: a group waits until every company has marched in." },
    Section { key: "modules.warfare.sieges", label: "Sieges in depth", help: "Besieged provinces raise no soldiers, lose their trade and stores, and fall faster when starving. Off: a siege only counts days." },
    // --- end of logistics ---
    // --- Research ---
    // --- end of research ---
    // --- Culture ---
    // --- end of culture ---
];

/// What in the configuration isn't a setting here, and why: a key, or the start of keys.
pub static LEFT_OUT: &[(&str, &str)] = &[
    ("world.seed", "which world it is, not how it works: New world has its own box for it"),
    ("calendar", "the calendar is content: months and their lengths"),
    ("goods", "the goods tree is content, changed through the content tables"),
    ("modules.terrain", "the version, picked from its own list"),
    ("modules.climate", "the version, picked from its own list"),
    ("modules.weather", "the version (droughts or none), picked from its own list"),
    ("modules.markets.money", "a name in the goods tree (which goods are coins), not a number"),
    ("modules.markets.tools", "a name in the goods tree (which good is tools), not a number"),
    ("modules.markets.goods.*.group", "which goods are priced together: part of the goods' content"),
    ("modules.markets.goods.*.harvested", "how the yearly table counts it: part of the goods' content"),
    ("modules.markets.goods.*.season", "whether it comes with the harvest: part of the goods' content"),
    ("modules.markets.goods.*.weather", "whether rain changes it: part of the goods' content"),
    ("modules.markets.goods.*.land", "what land makes it, as a formula over province values: part of the goods' content"),
    ("modules.markets.goods.*.lost_as", "what its losses are called: part of the goods' content"),
    ("modules.warfare.money", "a name in the goods tree (which goods are coins), not a number"),
    ("modules.warfare.fallback", "a name in the troop tree (the kind raised when goods run short), not a number"),
    ("modules.warfare.sea", "a name in the troop tree (which kinds are ships), not a number"),
    ("modules.warfare.tree", "the troop tree is content, changed through the content tables"),
    ("modules.warfare.kinds.*.land", "what land suits the kind: part of the troops' content"),
    ("modules.warfare.kinds.*.coast", "whether only coasts raise it: part of the troops' content"),
    ("modules.warfare.kinds.*.good", "which good the kind uses up: part of the troops' content"),
    // The first wave of modules built side by side (Oct 17): a block each.
    // --- Logistics and support ---
    // --- end of logistics ---
    // --- Research ---
    ("modules.research.fields", "the fields of knowledge are names: part of the research tree's content"),
    ("modules.research.techs.*.field", "which field a tech is in: part of the research tree's content"),
    ("modules.research.techs.*.needs", "which techs a tech needs first: part of the research tree's content"),
    // --- end of research ---
    // --- Culture ---
    // --- end of culture ---
];

/// One setting of a configuration, as it stands.
#[derive(Clone, Debug)]
pub struct Item {
    /// Its full key, with any `*` filled in.
    pub key: String,
    pub spec: &'static Spec,
    pub value: f64,
    /// Whether it holds whole numbers only.
    pub whole: bool,
    /// What each `*` of the key stands for here ("grain"), joined by " → ".
    pub of: String,
}

impl Item {
    /// The setting's line: its label, after what it is for if its key had a `*`.
    pub fn label(&self) -> String {
        if self.of.is_empty() { self.spec.label.to_string() } else { format!("{}: {}", self.of, self.spec.label) }
    }
}

/// Whether `key` is one the pattern `spec` (with `*` for any one part) stands for.
pub fn matches(spec: &str, key: &str) -> bool {
    let (mut a, mut b) = (spec.split('.'), key.split('.'));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x == "*" || x == y => {}
            _ => return false,
        }
    }
}

/// The description of the setting at `key`.
pub fn spec(key: &str) -> Option<&'static Spec> {
    GROUPS.iter().flat_map(|g| g.specs).find(|s| matches(s.key, key))
}

/// The group a setting's key belongs to.
pub fn group_of(key: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.specs.iter().any(|s| matches(s.key, key)))
}

/// Every module switched on, with every part: the configuration whose values every setting and
/// [`LEFT_OUT`] together must cover, and where a part switched on gets its values from. Both
/// economies are on, which no world runs, so that both are covered. Every field is named, so a new
/// module doesn't build until it is switched on here (a test checks the parts inside modules).
pub fn everything() -> ModuleConfig {
    let d = ModuleConfig::default();
    ModuleConfig {
        terrain: d.terrain,
        climate: d.climate,
        economics: Some(FoodParams::default()),
        population: Some(PopParams::default()),
        weather: true,
        markets: Some(MarketParams::default()),
        realms: Some(RealmParams::default()),
        characters: Some(Default::default()),
        diplomacy: Some(WarParams { treaties: Some(TreatyParams::default()), ..WarParams::default() }),
        warfare: Some(TroopParams {
            supply: Some(Default::default()),
            seasons: Some(Default::default()),
            pay: Some(Default::default()),
            sieges: Some(Default::default()),
            massing: Some(Default::default()),
            ..TroopParams::default()
        }),
        research: Some(ResearchParams::default()),
        culture: Some(CultureParams::default()),
        terrain_params: d.terrain_params,
        climate_params: d.climate_params,
        biome_params: d.biome_params,
        river_params: d.river_params,
        farmland_params: d.farmland_params,
        province_params: d.province_params,
    }
}

/// The same world with every setting at its default: the same versions, the same modules and parts
/// switched on, and the same map detail, province count, seed, calendar and goods (which the
/// world's size or content decide, not a setting's default).
pub fn defaults_for(c: &Config) -> Config {
    fn on<T>(x: bool, p: Option<T>) -> Option<T> {
        p.filter(|_| x)
    }
    let (m, all) = (&c.modules, everything());
    let mut markets = on(m.markets.is_some(), all.markets);
    if let Some(p) = &mut markets
        && m.markets.as_ref().is_some_and(|x| x.trade.is_none())
    {
        p.trade = None;
    }
    let mut realms = on(m.realms.is_some(), all.realms);
    if let Some(p) = &mut realms
        && m.realms.as_ref().is_some_and(|x| x.unrest.is_none())
    {
        p.unrest = None;
    }
    let mut diplomacy = on(m.diplomacy.is_some(), all.diplomacy);
    if let Some(p) = &mut diplomacy
        && m.diplomacy.as_ref().is_some_and(|x| x.treaties.is_none())
    {
        p.treaties = None;
    }
    let mut warfare = on(m.warfare.is_some(), all.warfare);
    if let (Some(p), Some(x)) = (&mut warfare, &m.warfare) {
        p.supply = p.supply.take().filter(|_| x.supply.is_some());
        p.seasons = p.seasons.take().filter(|_| x.seasons.is_some());
        p.pay = p.pay.take().filter(|_| x.pay.is_some());
        p.sieges = p.sieges.take().filter(|_| x.sieges.is_some());
        p.massing = p.massing.take().filter(|_| x.massing.is_some());
    }
    // --- Research: its parts ---
    let research = on(m.research.is_some(), all.research);
    // --- end of research ---
    // --- Culture: its parts ---
    let culture = on(m.culture.is_some(), all.culture);
    // --- end of culture ---
    let world = WorldParams { n: c.world.n, provinces: c.world.provinces, seed: c.world.seed, ..WorldParams::preset(Size::Small, 0) };
    let modules = ModuleConfig {
        terrain: m.terrain,
        climate: m.climate,
        weather: m.weather,
        economics: on(m.economics.is_some(), all.economics),
        population: on(m.population.is_some(), all.population),
        markets,
        realms,
        characters: on(m.characters.is_some(), all.characters),
        diplomacy,
        warfare,
        research,
        culture,
        ..ModuleConfig::default()
    };
    Config { world, modules, calendar: c.calendar.clone(), goods: c.goods.clone() }
}

/// The configuration as TOML values. The seed is left out (as 0): it can be larger than TOML's
/// integers, and isn't a setting.
fn tree(c: &Config) -> Value {
    let mut c = c.clone();
    c.world.seed = 0;
    Value::try_from(&c).expect("configurations serialise")
}

fn from_tree(v: Value, seed: u64) -> Result<Config, String> {
    let mut c: Config = v.try_into().map_err(|e| format!("the settings don't fit together: {e}"))?;
    c.world.seed = seed;
    Ok(c)
}

fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |v, k| v.get(k))
}

fn at_mut<'a>(v: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    path.split('.').try_fold(v, |v, k| v.get_mut(k))
}

fn number(v: &Value) -> Option<(f64, bool)> {
    match v {
        Value::Integer(i) => Some((*i as f64, true)),
        Value::Float(x) => Some((*x, false)),
        _ => None,
    }
}

/// Every key under `v` (at `path`) that `pattern` stands for, with what each `*` stood for.
fn expand(v: &Value, path: &str, pattern: &[&str], of: &[String], out: &mut Vec<(String, Vec<String>)>) {
    let Some((first, rest)) = pattern.split_first() else {
        out.push((path.to_string(), of.to_vec()));
        return;
    };
    let join = |k: &str| if path.is_empty() { k.to_string() } else { format!("{path}.{k}") };
    let Value::Table(t) = v else { return };
    if *first == "*" {
        for (k, sub) in t {
            let mut of = of.to_vec();
            of.push(k.clone());
            expand(sub, &join(k), rest, &of, out);
        }
    } else if let Some(sub) = t.get(*first) {
        expand(sub, &join(first), rest, of, out);
    }
}

/// Every setting the configuration has, group by group, in the order the groups list them. A
/// module switched off has none.
pub fn items(c: &Config) -> Vec<Item> {
    let v = tree(c);
    let mut out = Vec::new();
    for s in GROUPS.iter().flat_map(|g| g.specs) {
        let pattern: Vec<&str> = s.key.split('.').collect();
        let mut keys = Vec::new();
        expand(&v, "", &pattern, &[], &mut keys);
        for (key, of) in keys {
            if let Some((value, whole)) = at(&v, &key).and_then(number) {
                out.push(Item { key, spec: s, value, whole, of: of.join(" → ") });
            }
        }
    }
    out
}

/// The value of the setting at `key`, if the configuration has it.
pub fn get(c: &Config, key: &str) -> Option<f64> {
    at(&tree(c), key).and_then(number).map(|x| x.0)
}

/// Set the setting at `key`. Refused, with the reason, for a key that is no setting or that this
/// configuration doesn't have, and for a value outside the setting's range (or not whole, where
/// it holds whole numbers). Whether the values fit together (an uprising dying down below where
/// provinces join it, say) is the modules' check, when the world is made or the change applied.
pub fn set(c: &mut Config, key: &str, value: f64) -> Result<(), String> {
    let s = spec(key).ok_or_else(|| format!("{key} is not a setting"))?;
    if !(value.is_finite() && s.min <= value && value <= s.max) {
        return Err(format!("{}: {value} is outside {} to {}", s.label, s.min, s.max));
    }
    let mut v = tree(c);
    let slot = at_mut(&mut v, key).ok_or_else(|| format!("this world has no {key}"))?;
    *slot = match slot {
        Value::Integer(_) if value.fract() != 0.0 => return Err(format!("{}: {value} is not a whole number", s.label)),
        Value::Integer(_) => Value::Integer(value as i64),
        Value::Float(_) => Value::Float(value),
        _ => return Err(format!("{key} is not a number")),
    };
    *c = from_tree(v, c.world.seed)?;
    Ok(())
}

/// Whether the part at `key` (one of [`SECTIONS`]) is on.
pub fn section_on(c: &Config, key: &str) -> bool {
    at(&tree(c), key).is_some()
}

/// Switch the part at `key` (one of [`SECTIONS`]) on, with its default values, or off. Its module
/// must be on.
pub fn set_section(c: &mut Config, key: &str, on: bool) -> Result<(), String> {
    if !SECTIONS.iter().any(|s| s.key == key) {
        return Err(format!("{key} is not a part to switch on or off"));
    }
    let (parent, name) = key.rsplit_once('.').expect("sections are inside modules");
    let mut v = tree(c);
    let mut all = c.clone();
    all.modules = everything();
    let default = at(&tree(&all), key).cloned().expect("everything has every part");
    let Some(Value::Table(t)) = at_mut(&mut v, parent) else { return Err(format!("{parent} is switched off")) };
    if on {
        t.entry(name).or_insert(default);
    } else {
        t.remove(name);
    }
    *c = from_tree(v, c.world.seed)?;
    Ok(())
}

/// The settings of `c` that differ from `base`'s (by key), for "changed from the defaults".
pub fn changed(c: &Config, base: &Config) -> Vec<Item> {
    let theirs = items(base);
    items(c).into_iter().filter(|i| theirs.iter().find(|t| t.key == i.key).is_none_or(|t| t.value != i.value)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_on() -> Config {
        let mut c = Config::new(WorldParams::preset(Size::Small, 1));
        c.modules = everything();
        c
    }

    /// Every number, name and switch under `v`, by key.
    fn leaves(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Table(t) => {
                for (k, sub) in t {
                    leaves(sub, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out);
                }
            }
            _ => out.push(path.to_string()),
        }
    }

    fn left_out(key: &str) -> bool {
        LEFT_OUT.iter().any(|(p, _)| matches(p, key) || key.split('.').enumerate().any(|(i, _)| matches(p, &key.split('.').take(i + 1).collect::<Vec<_>>().join("."))))
    }

    /// The rule for every module: each value in its configuration is a setting described here, or
    /// is left out here with the reason. A new parameter without either fails this.
    #[test]
    fn every_value_is_a_setting_or_left_out_with_a_reason() {
        let mut keys = Vec::new();
        leaves(&tree(&all_on()), "", &mut keys);
        let missing: Vec<&String> = keys.iter().filter(|k| spec(k).is_none() && !left_out(k)).collect();
        assert!(missing.is_empty(), "values with no description in settings.rs (describe them in GROUPS, or say in LEFT_OUT why not): {missing:?}");
        for (k, why) in LEFT_OUT {
            assert!(!why.is_empty(), "{k}");
        }
    }

    /// Parts inside a module that are off by default (`None`) would be missed by the test above:
    /// they don't serialise. Every one must be on in [`everything`], or be a value left out.
    #[test]
    fn every_part_is_on_in_everything() {
        assert!(ron::to_string(&WarParams::default()).unwrap().contains("treaties:None"), "how RON writes a part that is off");
        let text = ron::to_string(&everything()).unwrap();
        let allowed: Vec<&str> = LEFT_OUT.iter().map(|(k, _)| k.rsplit('.').next().unwrap()).collect();
        for (at, _) in text.match_indices(":None") {
            let field = text[..at].rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap();
            assert!(allowed.contains(&field), "{field} is off in everything(): switch it on there (and list it in SECTIONS if it can be switched)");
        }
    }

    #[test]
    fn every_description_names_a_value_and_holds_its_default() {
        let c = all_on();
        let items = items(&c);
        for s in GROUPS.iter().flat_map(|g| g.specs) {
            let of: Vec<&Item> = items.iter().filter(|i| std::ptr::eq(i.spec, s)).collect();
            assert!(!of.is_empty(), "{} names no value of the configuration", s.key);
            assert!(!s.label.is_empty() && s.help.len() > 20, "{} needs a label and an explanation", s.key);
            assert!(s.min <= s.max, "{}", s.key);
            for i in of {
                assert!(s.min <= i.value && i.value <= s.max, "{}: the default {} is outside {} to {}", i.key, i.value, s.min, s.max);
            }
            let g = group_of(s.key).expect("in a group");
            assert!(s.key.starts_with(g.at), "{} is outside its group's {}", s.key, g.at);
        }
        for s in SECTIONS {
            assert!(section_on(&c, s.key), "{}", s.key);
        }
    }

    #[test]
    fn settings_are_read_and_set_by_key() {
        let mut c = all_on();
        assert_eq!(get(&c, "modules.markets.trade.carriage.grain"), Some(0.02));
        set(&mut c, "modules.markets.trade.carriage.grain", 0.05).unwrap();
        assert_eq!(c.modules.markets.as_ref().unwrap().trade.as_ref().unwrap().carriage["grain"], 0.05);
        set(&mut c, "world.provinces", 800.0).unwrap();
        assert_eq!(c.world.provinces, 800);
        assert!(set(&mut c, "world.provinces", 800.5).unwrap_err().contains("whole"));
        assert!(set(&mut c, "modules.markets.trade.change", 2.0).unwrap_err().contains("outside"));
        assert!(set(&mut c, "world.seed", 3.0).unwrap_err().contains("not a setting"));
        // A huge seed survives a change.
        c.world.seed = u64::MAX;
        set(&mut c, "modules.realms.unrest.breakaway", 5.0).unwrap();
        assert_eq!((c.world.seed, c.modules.realms.as_ref().unwrap().unrest.as_ref().unwrap().breakaway), (u64::MAX, 5));
        let base = all_on();
        assert_eq!(changed(&c, &base).len(), 3);
        assert_eq!(changed(&c, &defaults_for(&c)).len(), 2, "the province count is the world size's");
        assert!(changed(&base, &defaults_for(&base)).is_empty());
        // A module switched off has no settings.
        c.modules.markets = None;
        assert!(set(&mut c, "modules.markets.trade.change", 0.3).unwrap_err().contains("has no"));
    }

    #[test]
    fn parts_switch_on_with_their_defaults_and_off() {
        let mut c = Config::new(WorldParams::preset(Size::Small, 1));
        c.modules.realms = Some(RealmParams { unrest: None, ..RealmParams::default() });
        assert!(!section_on(&c, "modules.realms.unrest"));
        set_section(&mut c, "modules.realms.unrest", true).unwrap();
        assert_eq!(c.modules.realms.as_ref().unwrap().unrest, RealmParams::default().unrest);
        set_section(&mut c, "modules.realms.unrest", false).unwrap();
        assert_eq!(c.modules.realms.as_ref().unwrap().unrest, None);
        assert!(set_section(&mut c, "modules.markets.trade", true).unwrap_err().contains("switched off"));
    }
}
